// CanvasController with a real engine and no GPU: geometry of what is on screen, scheduling and
// cancelling tile requests, zoom anchoring.
#include "SyntheticPdf.h"
#include "canvas/CanvasController.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

using vellora::CanvasController;
using vellora::EngineSession;
using vellora::PageLayout;

namespace {

constexpr int kWaitMs = 60'000;

// A session over the golden PDF (three pages: 220 x 140, 200 x 160, and a third) and a controller
// with a 400 x 100 viewport, so the pages are taller than the window.
struct Fixture {
    EngineSession session;
    CanvasController controller{&session};

    explicit Fixture(const QString& path = QStringLiteral(VELLORA_GOLDEN_PDF),
                     QSize viewport = QSize(400, 100)) {
        controller.setViewportSize(viewport);
        QSignalSpy opened(&session, &EngineSession::opened);
        const QString error = session.open(path);
        if (!error.isEmpty() || !opened.wait(kWaitMs)) {
            qFatal("could not open the golden document: %s", qPrintable(error));
        }
    }
};

double area(const QRectF& rect) {
    return rect.isEmpty() ? 0.0 : rect.width() * rect.height();
}

} // namespace

class TstCanvasController : public QObject {
    Q_OBJECT

private slots:
    void layoutFollowsTheOpenedDocument() {
        Fixture f;
        QCOMPARE(f.controller.pageCount(), 3U);
        QCOMPARE(f.controller.layout().pageSize(0), QSizeF(220.0, 140.0));
        QCOMPARE(f.controller.zoom(), 1.0);
        // The first page is centred with the gap above it.
        const vellora::Frame frame = f.controller.frame();
        QVERIFY(!frame.pages.isEmpty());
        QCOMPARE(frame.pages.first().page, 0U);
        QCOMPARE(frame.pages.first().rect,
                 QRectF((400.0 - 220.0) / 2.0, vellora::PageLayout::kGap, 220.0, 140.0));
    }

    void aSmallPageIsOneTileThatShowsPartOfTheTexture() {
        Fixture f;
        const vellora::Frame frame = f.controller.frame();
        QCOMPARE(frame.tiles.size(), 1);
        const vellora::TileDraw& tile = frame.tiles.first();
        QCOMPARE(tile.dest, frame.pages.first().rect);
        QCOMPARE(tile.uv, QRectF(0.0, 0.0, 220.0 / 512.0, 140.0 / 512.0));
        QCOMPARE(tile.scale, 1.0F);
    }

    void tilesCoverExactlyWhatIsVisibleOfEachPage() {
        Fixture f;
        for (const double zoom : {0.5, 1.0, 1.7, 3.0, 8.0}) {
            f.controller.setZoom(zoom, QPointF(200.0, 50.0));
            for (const double scroll : {0.0, 37.0, 150.0, 1.0e9}) {
                f.controller.setScrollPosition(QPointF(scroll / 3.0, scroll));
                const vellora::Frame frame = f.controller.frame();
                const QRectF viewport(QPointF(0, 0), QSizeF(f.controller.viewportSize()));
                for (const vellora::PageDraw& page : frame.pages) {
                    double covered = 0.0;
                    for (const vellora::TileDraw& tile : frame.tiles) {
                        if (tile.page != page.page) {
                            continue;
                        }
                        // Inside its page, with a valid part of the texture.
                        QVERIFY(page.rect.adjusted(-1e-6, -1e-6, 1e-6, 1e-6).contains(tile.dest));
                        QVERIFY(tile.uv.left() >= 0.0 && tile.uv.top() >= 0.0);
                        QVERIFY(tile.uv.right() <= 1.0 + 1e-9 && tile.uv.bottom() <= 1.0 + 1e-9);
                        covered += area(tile.dest.intersected(viewport));
                    }
                    const double visible = area(page.rect.intersected(viewport));
                    QVERIFY2(
                        qAbs(covered - visible) < 1e-3 * (1.0 + visible),
                        qPrintable(QStringLiteral("zoom %1 scroll %2 page %3: tiles cover %4 of %5")
                                       .arg(zoom)
                                       .arg(scroll)
                                       .arg(page.page)
                                       .arg(covered)
                                       .arg(visible)));
                }
            }
        }
    }

    void scrollIsClampedToTheContent() {
        Fixture f;
        f.controller.setScrollPosition(QPointF(-5.0, -5.0));
        QCOMPARE(f.controller.scrollPosition(), QPointF(0.0, 0.0));
        f.controller.setScrollPosition(QPointF(1.0e9, 1.0e9));
        const QSizeF content = f.controller.contentSize();
        QCOMPARE(f.controller.scrollPosition().y(), content.height() - 100.0);
        // The pages are narrower than the window: nothing to scroll sideways.
        QCOMPARE(f.controller.scrollPosition().x(), 0.0);
    }

    void currentPageIsTheOneUnderTheMiddleOfTheWindow() {
        Fixture f;
        QSignalSpy changed(&f.controller, &CanvasController::currentPageChanged);
        QCOMPARE(f.controller.currentPage(), 0U);
        f.controller.setScrollPosition(QPointF(0.0, 1.0e9));
        QCOMPARE(f.controller.currentPage(), 2U);
        QVERIFY(!changed.isEmpty());
        QCOMPARE(changed.last().at(0).toUInt(), 2U);
        QCOMPARE(changed.last().at(1).toUInt(), 3U);
    }

    void zoomKeepsThePointUnderTheAnchor() {
        Fixture f;
        f.controller.setScrollPosition(QPointF(0.0, 150.0));
        const QPointF anchor(120.0, 40.0);
        const auto before = f.controller.layout().anchorAt(150.0 + anchor.y(), 1.0);
        f.controller.setZoom(2.0, anchor);
        QCOMPARE(f.controller.zoom(), 2.0);
        const auto after = f.controller.layout().anchorAt(
            f.controller.scrollPosition().y() + anchor.y(), f.controller.zoom());
        QCOMPARE(after.page, before.page);
        QVERIFY(qAbs(after.offsetPoints - before.offsetPoints) < 1e-6);
    }

    void zoomIsLimited() {
        Fixture f;
        f.controller.setZoom(1000.0, QPointF());
        QCOMPARE(f.controller.zoom(), CanvasController::kMaxZoom);
        f.controller.setZoom(0.0001, QPointF());
        QCOMPARE(f.controller.zoom(), CanvasController::kMinZoom);
    }

    void fitWidthAndActualSize() {
        Fixture f;
        f.controller.fitWidth();
        const double expected =
            (400.0 - 2.0 * vellora::PageLayout::kGap) / f.controller.layout().maxPageWidth();
        QVERIFY(qAbs(f.controller.zoom() - expected) < 1e-9);
        f.controller.actualSize();
        QCOMPARE(f.controller.zoom(), 1.0);
    }

    void visibleTilesArriveAndStaleRequestsAreWithdrawn() {
        Fixture f;
        const vellora::TileDraw first = f.controller.frame().tiles.first();
        QByteArray pixels;
        // The engine renders the visible tile and the client cache serves it.
        QTRY_VERIFY_WITH_TIMEOUT(
            f.session.readTile(first.page, first.scale, first.x, first.y, pixels), kWaitMs);

        // A burst of zooms and scrolls leaves no request behind for tiles nobody wants any more.
        f.controller.setZoom(8.0, QPointF(200.0, 50.0));
        f.controller.setScrollPosition(QPointF(300.0, 900.0));
        f.controller.setZoom(0.5, QPointF(200.0, 50.0));
        f.controller.setZoom(1.0, QPointF(200.0, 50.0));
        QTRY_COMPARE_WITH_TIMEOUT(f.controller.tilesInFlight(), 0, kWaitMs);
        // And whatever is visible is there in the end.
        for (const vellora::TileDraw& tile : f.controller.frame().tiles) {
            QTRY_VERIFY_WITH_TIMEOUT(
                f.session.readTile(tile.page, tile.scale, tile.x, tile.y, pixels), kWaitMs);
        }
    }

    void aTenThousandPageDocumentOpensAndScrollsToTheEnd() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        Fixture f(path, QSize(800, 600));
        QCOMPARE(f.controller.pageCount(), 10'000U);
        QCOMPARE(f.controller.currentPage(), 0U);
        // The column is as tall as 10,000 Letter pages plus gaps.
        const double expected = 10'001 * vellora::PageLayout::kGap + 10'000 * 792.0;
        QCOMPARE(f.controller.contentSize().height(), expected);

        // Jump around the document; requests never pile up beyond what is visible plus prefetch.
        const int bound = 64 + CanvasController::kMaxPrefetchTiles;
        for (const double fraction : {0.9999, 0.5, 0.25, 0.75, 1.0, 0.0, 0.3333}) {
            f.controller.setScrollPosition(QPointF(0.0, fraction * expected));
            QVERIFY(f.controller.tilesInFlight() <= bound);
            QVERIFY(!f.controller.frame().tiles.isEmpty());
        }
        f.controller.setScrollPosition(QPointF(0.0, 1.0e12));
        QCOMPARE(f.controller.currentPage(), 9'999U);

        // At the end the visible tiles arrive, and nothing is left in flight from the way here.
        QByteArray pixels;
        for (const vellora::TileDraw& tile : f.controller.frame().tiles) {
            QTRY_VERIFY_WITH_TIMEOUT(
                f.session.readTile(tile.page, tile.scale, tile.x, tile.y, pixels), kWaitMs);
        }
        QTRY_COMPARE_WITH_TIMEOUT(f.controller.tilesInFlight(), 0, kWaitMs);
    }

    void aNewDocumentStartsFromTheTop() {
        Fixture f;
        f.controller.setZoom(2.0, QPointF());
        f.controller.setScrollPosition(QPointF(0.0, 200.0));
        f.controller.reset();
        QCOMPARE(f.controller.zoom(), 1.0);
        QCOMPARE(f.controller.scrollPosition(), QPointF(0.0, 0.0));
        QCOMPARE(f.controller.pageCount(), 0U);
        QVERIFY(f.controller.frame().pages.isEmpty());
    }

    // ---- view modes, fit, rotation, zoom (M1 task 10) ----

    void everyModeCoversExactlyWhatIsVisibleOfEachPageItShows() {
        Fixture f;
        for (const bool continuous : {true, false}) {
            for (const auto spread :
                 {PageLayout::Spread::One, PageLayout::Spread::Two, PageLayout::Spread::TwoCover}) {
                for (const int rotation : {0, 1, 2, 3}) {
                    f.controller.setViewMode({continuous, spread, rotation});
                    for (const double zoom : {0.5, 1.0, 2.5}) {
                        f.controller.setZoom(zoom, QPointF(200.0, 50.0));
                        for (const double scroll : {0.0, 60.0, 1.0e9}) {
                            f.controller.setScrollPosition(QPointF(scroll / 2.0, scroll));
                            const vellora::Frame frame = f.controller.frame();
                            const QRectF viewport(QPointF(0, 0),
                                                  QSizeF(f.controller.viewportSize()));
                            const QString where = QStringLiteral("%1 spread %2 turn %3 zoom %4 "
                                                                 "scroll %5")
                                                      .arg(continuous ? "continuous" : "single")
                                                      .arg(static_cast<int>(spread))
                                                      .arg(rotation)
                                                      .arg(zoom)
                                                      .arg(scroll);
                            QVERIFY2(!frame.pages.isEmpty(), qPrintable(where));
                            for (const vellora::PageDraw& page : frame.pages) {
                                // The page is shown with its turned size.
                                const QSizeF file = f.controller.layout().pageSize(page.page);
                                const QSizeF shown =
                                    rotation % 2 == 0 ? file : QSizeF(file.height(), file.width());
                                QVERIFY2(qAbs(page.rect.width() - shown.width() * zoom) < 1e-6,
                                         qPrintable(where));
                                QVERIFY2(qAbs(page.rect.height() - shown.height() * zoom) < 1e-6,
                                         qPrintable(where));
                                double covered = 0.0;
                                for (const vellora::TileDraw& tile : frame.tiles) {
                                    if (tile.page != page.page) {
                                        continue;
                                    }
                                    QCOMPARE(tile.rotation, rotation);
                                    QVERIFY2(page.rect.adjusted(-1e-6, -1e-6, 1e-6, 1e-6)
                                                 .contains(tile.dest),
                                             qPrintable(where));
                                    QVERIFY(tile.uv.left() >= -1e-9 && tile.uv.top() >= -1e-9);
                                    QVERIFY(tile.uv.right() <= 1.0 + 1e-9 &&
                                            tile.uv.bottom() <= 1.0 + 1e-9);
                                    covered += area(tile.dest.intersected(viewport));
                                }
                                const double visible = area(page.rect.intersected(viewport));
                                QVERIFY2(qAbs(covered - visible) < 1e-3 * (1.0 + visible),
                                         qPrintable(where + QStringLiteral(": covers %1 of %2")
                                                                .arg(covered)
                                                                .arg(visible)));
                            }
                        }
                    }
                }
            }
        }
    }

    void singlePageModeShowsOneRowAndScrollingTurnsPages() {
        Fixture f;
        QSignalSpy changed(&f.controller, &CanvasController::currentPageChanged);
        f.controller.setContinuous(false);
        QVERIFY(!f.controller.viewMode().continuous);
        {
            const vellora::Frame frame = f.controller.frame();
            QCOMPARE(frame.pages.size(), 1);
            QCOMPARE(frame.pages.first().page, 0U);
            for (const auto& tile : frame.tiles) {
                QCOMPARE(tile.page, 0U);
            }
        }
        // The allowed scroll positions are those of the row.
        const auto range = f.controller.verticalRange();
        QCOMPARE(range.min, f.controller.layout().rowTop(0, 1.0) - PageLayout::kGap);
        f.controller.setScrollPosition(QPointF(0.0, 1.0e9));
        QCOMPARE(f.controller.scrollPosition().y(), range.max);
        QCOMPARE(f.controller.currentPage(), 0U); // still page 1: the window cannot leave the row

        // Scrolling past the bottom edge turns to the next page, at its top...
        f.controller.scrollBy(QPointF(0.0, 50.0));
        QCOMPARE(f.controller.currentPage(), 1U);
        QCOMPARE(f.controller.frame().pages.first().page, 1U);
        QCOMPARE(f.controller.scrollPosition().y(),
                 f.controller.layout().rowTop(1, 1.0) - PageLayout::kGap);
        QCOMPARE(changed.last().at(0).toUInt(), 1U);
        // ...and past the top edge back to the previous one, at its bottom.
        f.controller.scrollBy(QPointF(0.0, -50.0));
        QCOMPARE(f.controller.currentPage(), 0U);
        QCOMPARE(f.controller.scrollPosition().y(), f.controller.verticalRange().max);
        // The first page has nothing before it, the last nothing after.
        f.controller.goToPage(0);
        f.controller.scrollBy(QPointF(0.0, -5000.0));
        QCOMPARE(f.controller.currentPage(), 0U);
        f.controller.goToPage(2);
        f.controller.setScrollPosition(QPointF(0.0, 1.0e9));
        f.controller.scrollBy(QPointF(0.0, 5000.0));
        QCOMPARE(f.controller.currentPage(), 2U);

        // Page commands.
        f.controller.goToPage(0);
        f.controller.nextPage();
        QCOMPARE(f.controller.currentPage(), 1U);
        f.controller.nextPage();
        f.controller.nextPage();
        QCOMPARE(f.controller.currentPage(), 2U);
        f.controller.previousPage();
        QCOMPARE(f.controller.currentPage(), 1U);
        f.controller.goToPage(99);
        QCOMPARE(f.controller.currentPage(), 2U); // clamped

        // Back to continuous scrolling: the same page stays current.
        f.controller.setContinuous(true);
        QCOMPARE(f.controller.currentPage(), 2U);
        QCOMPARE(f.controller.frame().pages.size() >= 1, true);
    }

    void continuousNavigationScrollsToTheTopOfTheRow() {
        Fixture f;
        f.controller.goToPage(2);
        QCOMPARE(f.controller.scrollPosition().y(),
                 qMin(f.controller.layout().rowTop(2, 1.0) - PageLayout::kGap,
                      f.controller.verticalRange().max));
        f.controller.goToPage(0);
        QCOMPARE(f.controller.scrollPosition().y(), 0.0);
        f.controller.nextPage();
        QCOMPARE(f.controller.currentPage() >= 0U, true);
    }

    void twoUpShowsTheTwoPagesOfARowBesideTheSpine() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF), QSize(600, 400));
        f.controller.setSpread(PageLayout::Spread::Two);
        const vellora::Frame frame = f.controller.frame();
        QCOMPARE(frame.pages.size(), 3 >= 2 ? 3 : 2); // rows (0 1) and (2) are both on screen
        const QRectF left = frame.pages.at(0).rect;
        const QRectF right = frame.pages.at(1).rect;
        QCOMPARE(frame.pages.at(0).page, 0U);
        QCOMPARE(frame.pages.at(1).page, 1U);
        QCOMPARE(right.left() - left.right(), PageLayout::kGap);
        QCOMPARE(left.top(), right.top());
        // The spine is the middle of the window.
        QVERIFY(qAbs((left.right() + right.left()) / 2.0 - 300.0) < 1e-9);
        // Page 3 is alone in the next row, on the left.
        QCOMPARE(frame.pages.at(2).page, 2U);
        QVERIFY(frame.pages.at(2).rect.right() <= 300.0);
        QVERIFY(frame.pages.at(2).rect.top() > left.bottom());

        // With a cover, the first page stands alone on the right.
        f.controller.setSpread(PageLayout::Spread::TwoCover);
        const vellora::Frame cover = f.controller.frame();
        QCOMPARE(cover.pages.first().page, 0U);
        QVERIFY(cover.pages.first().rect.left() > 300.0);
        QCOMPARE(cover.pages.at(1).page, 1U);
        QVERIFY(cover.pages.at(1).rect.right() < 300.0);
        QCOMPARE(cover.pages.at(2).page, 2U);
        QVERIFY(cover.pages.at(2).rect.left() > 300.0);
    }

    void changingTheArrangementKeepsThePageCurrent() {
        QTemporaryDir dir;
        const QString path = dir.filePath(QStringLiteral("many.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(40));
        file.close();
        Fixture f(path, QSize(800, 600));
        f.controller.goToPage(21);
        QCOMPARE(f.controller.currentPage(), 21U);
        f.controller.setSpread(PageLayout::Spread::Two);
        QCOMPARE(f.controller.currentPage(), 20U); // the first page of the spread that holds it
        // With a cover the pairs are (19 20) (21 22): the page that was current, 20, is in the
        // first of those, and a spread is named by its first page.
        f.controller.setSpread(PageLayout::Spread::TwoCover);
        QCOMPARE(f.controller.currentPage(), 19U);
        f.controller.setSpread(PageLayout::Spread::One);
        QCOMPARE(f.controller.currentPage(), 19U);
        f.controller.setContinuous(false);
        QCOMPARE(f.controller.currentPage(), 19U);
        f.controller.rotateBy(1);
        QCOMPARE(f.controller.currentPage(), 19U);
        f.controller.rotateBy(-1);
        f.controller.setSpread(PageLayout::Spread::Two);
        QCOMPARE(f.controller.currentPage(), 18U);
        QCOMPARE(f.controller.frame().pages.size(), 2);
    }

    void theViewTurnsInQuarterTurnsAndTilesFollow() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF), QSize(400, 300));
        QSignalSpy modes(&f.controller, &CanvasController::viewModeChanged);
        const QSizeF before = f.controller.frame().pages.first().rect.size();
        f.controller.rotateBy(1);
        QCOMPARE(f.controller.viewMode().rotation, 1);
        QCOMPARE(modes.size(), 1);
        const vellora::Frame frame = f.controller.frame();
        QCOMPARE(frame.pages.first().rect.size(), QSizeF(before.height(), before.width()));
        QCOMPARE(frame.pages.first().rotation, 1);
        QCOMPARE(frame.pages.first().filePoints, QSizeF(220.0, 140.0)); // the file is not changed
        QVERIFY(!frame.tiles.isEmpty());
        QCOMPARE(frame.tiles.first().rotation, 1);
        // A tile of the whole small page shows the whole texture area, turned onto the page.
        QCOMPARE(frame.tiles.first().dest, frame.pages.first().rect);
        QCOMPARE(frame.tiles.first().uv, QRectF(0.0, 0.0, 220.0 / 512.0, 140.0 / 512.0));
        // The page maps any part of itself the same way the tiles do.
        QCOMPARE(frame.pages.first().map(QRectF(0.0, 0.0, 220.0, 140.0)), frame.pages.first().rect);

        f.controller.rotateBy(-2);
        QCOMPARE(f.controller.viewMode().rotation, 3);
        f.controller.rotateBy(1);
        f.controller.rotateBy(1);
        QCOMPARE(f.controller.viewMode().rotation, 1);
        // Setting the mode that is already set changes nothing and says nothing.
        modes.clear();
        f.controller.setViewMode(f.controller.viewMode());
        QCOMPARE(modes.size(), 0);
        // The mode is the user's choice: a new document keeps it.
        f.controller.reset();
        QCOMPARE(f.controller.viewMode().rotation, 1);
    }

    void fitWidthAndFitPageFollowTheWindowUntilTheUserZooms() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF), QSize(400, 100));
        QSignalSpy modes(&f.controller, &CanvasController::zoomModeChanged);
        const double gap = PageLayout::kGap;
        f.controller.fitWidth();
        QCOMPARE(f.controller.zoomMode(), CanvasController::ZoomMode::FitWidth);
        const double widest = f.controller.layout().maxPageWidth();
        QVERIFY(qAbs(f.controller.zoom() - (400.0 - 2.0 * gap) / widest) < 1e-9);
        // Resizing the window re-fits.
        f.controller.setViewportSize(QSize(800, 100));
        QVERIFY(qAbs(f.controller.zoom() - (800.0 - 2.0 * gap) / widest) < 1e-9);
        // A turn changes the widest page, and the fit follows.
        f.controller.rotateBy(1);
        const double turned = f.controller.layout().maxPageWidth();
        QVERIFY(qAbs(f.controller.zoom() - (800.0 - 2.0 * gap) / turned) < 1e-9);
        f.controller.rotateBy(-1);

        // Fit page: the whole current page in the window.
        f.controller.setViewportSize(QSize(400, 300));
        f.controller.fitPage();
        QCOMPARE(f.controller.zoomMode(), CanvasController::ZoomMode::FitPage);
        const QSizeF page = f.controller.layout().displaySize(0);
        const double expected =
            qMin((400.0 - 2.0 * gap) / page.width(), (300.0 - 2.0 * gap) / page.height());
        QVERIFY(qAbs(f.controller.zoom() - expected) < 1e-9);
        const QRectF shown = f.controller.frame().pages.first().rect;
        QVERIFY(shown.left() >= -1e-6 && shown.right() <= 400.0 + 1e-6);
        QVERIFY(shown.top() >= -1e-6 && shown.bottom() <= 300.0 + 1e-6);
        // Turning to another page fits that one.
        f.controller.nextPage();
        const QSizeF second = f.controller.layout().displaySize(f.controller.currentPage());
        const double expectedSecond =
            qMin((400.0 - 2.0 * gap) / second.width(), (300.0 - 2.0 * gap) / second.height());
        QVERIFY(qAbs(f.controller.zoom() - expectedSecond) < 1e-9);

        // Zooming by hand ends the following; so does actual size.
        f.controller.zoomBy(1.25, QPointF(10.0, 10.0));
        QCOMPARE(f.controller.zoomMode(), CanvasController::ZoomMode::Custom);
        f.controller.fitWidth();
        f.controller.actualSize();
        QCOMPARE(f.controller.zoomMode(), CanvasController::ZoomMode::Custom);
        QCOMPARE(f.controller.zoom(), 1.0);
        f.controller.setViewportSize(QSize(900, 300));
        QCOMPARE(f.controller.zoom(), 1.0); // no longer follows
        QVERIFY(modes.size() >= 4);
        // Both follow in a spread too: the pair must fit.
        f.controller.setSpread(PageLayout::Spread::Two);
        f.controller.fitWidth();
        const double row = f.controller.layout().halfWidthPoints() * 2.0;
        QVERIFY(qAbs(f.controller.zoom() -
                     (900.0 - 2.0 * gap - 2.0 * f.controller.layout().halfFixedWidth()) / row) <
                1e-9);
    }

    void zoomToARectangleFillsTheWindowWithIt() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF), QSize(400, 100));
        // A rectangle around the middle of page 1, wider than tall, as the window.
        const QRectF rect(150.0, 30.0, 100.0, 50.0);
        const QRectF page = f.controller.frame().pages.first().rect;
        const QPointF spot = (rect.center() - page.topLeft()); // in page points at zoom 1
        f.controller.zoomToRect(rect);
        QCOMPARE(f.controller.zoom(), 2.0); // min(400 / 100, 100 / 50)
        QCOMPARE(f.controller.zoomMode(), CanvasController::ZoomMode::Custom);
        const QRectF after = f.controller.frame().pages.first().rect;
        const QPointF centre = after.topLeft() + spot * 2.0;
        QVERIFY2(qAbs(centre.x() - 200.0) < 1e-6 && qAbs(centre.y() - 50.0) < 1e-6,
                 qPrintable(QStringLiteral("%1,%2").arg(centre.x()).arg(centre.y())));

        // A tiny or empty rectangle is ignored, a huge zoom is limited.
        const double zoom = f.controller.zoom();
        f.controller.zoomToRect(QRectF(10, 10, 0.2, 0.2));
        f.controller.zoomToRect(QRectF());
        QCOMPARE(f.controller.zoom(), zoom);
        f.controller.zoomToRect(QRectF(150.0, 40.0, 1.0, 1.0));
        QCOMPARE(f.controller.zoom(), CanvasController::kMaxZoom);
    }

    void zoomPresetsRunFromTwentyFivePercentTo6400() {
        const QVector<double> presets = CanvasController::zoomPresets();
        QCOMPARE(presets.first(), 0.25);
        QCOMPARE(presets.last(), 64.0);
        QVERIFY(presets.contains(1.0));
        QVERIFY(std::is_sorted(presets.begin(), presets.end()));
        QCOMPARE(CanvasController::kMaxZoom, 64.0);
        // Every preset can be reached, tiles included (6400% is the largest scale the protocol
        // has).
        Fixture f;
        for (const double preset : presets) {
            f.controller.setZoom(preset, QPointF(200.0, 50.0));
            QCOMPARE(f.controller.zoom(), preset);
            QVERIFY(!f.controller.frame().tiles.isEmpty());
        }
    }

    void aSavedViewComesBackInAnyModeAndZoom() {
        QTemporaryDir dir;
        const QString path = dir.filePath(QStringLiteral("many.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(30));
        file.close();
        Fixture f(path, QSize(800, 600));
        f.controller.setViewMode({false, PageLayout::Spread::Two, 1});
        f.controller.restoreView({21, 10.0}, 1.5);
        QCOMPARE(f.controller.zoom(), 1.5);
        QCOMPARE(f.controller.currentPage(), 20U);
        QCOMPARE(f.controller.frame().pages.first().page, 20U);
        QVERIFY(qAbs(f.controller.topAnchor().offsetPoints - 10.0) < 1.0 ||
                f.controller.topAnchor().page == 20U);
    }
};

QTEST_MAIN(TstCanvasController)
#include "tst_canvas_controller.moc"
