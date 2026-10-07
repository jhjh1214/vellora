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
};

QTEST_MAIN(TstCanvasController)
#include "tst_canvas_controller.moc"
