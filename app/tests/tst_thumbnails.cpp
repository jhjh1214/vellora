// The thumbnail sidebar with a real engine and no GPU: what it asks for, what it draws, and how it
// follows and drives the canvas. Its frame-time check is `tst_thumbnails_frame_time`.
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"
#include "settings/AppSettings.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QFile>
#include <QScrollBar>
#include <QSettings>
#include <QSignalSpy>
#include <QSlider>
#include <QTemporaryDir>
#include <QTest>

using vellora::CanvasController;
using vellora::EngineSession;
using vellora::ThumbnailView;

namespace {

constexpr int kWaitMs = 60'000;

QString writeSynthetic(const QTemporaryDir& dir, int pages) {
    const QString path = dir.filePath(QStringLiteral("%1.pdf").arg(pages));
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly)) {
        qFatal("cannot write %s", qPrintable(path));
    }
    file.write(syntheticPdf(pages));
    return path;
}

// A session, a canvas controller with a 400 x 300 window and a thumbnail view 200 x 500, opened on
// `path`. The view is connected before the session opens, as in a tab.
struct Fixture {
    EngineSession session;
    CanvasController controller{&session};
    ThumbnailView view{&session, &controller};

    explicit Fixture(const QString& path = QStringLiteral(VELLORA_GOLDEN_PDF)) {
        controller.setViewportSize(QSize(400, 300));
        view.resize(200, 500);
        view.show();
        if (!QTest::qWaitForWindowExposed(&view)) {
            qFatal("the thumbnail view was not shown");
        }
        QSignalSpy opened(&session, &EngineSession::opened);
        const QString error = session.open(path);
        if (!error.isEmpty() || !opened.wait(kWaitMs)) {
            qFatal("could not open %s: %s", qPrintable(path), qPrintable(error));
        }
    }

    // The centre of the cell of `page`, in viewport pixels.
    QPoint centreOf(quint32 page) const { return view.cellRect(page).center(); }
};

} // namespace

class TstThumbnails : public QObject {
    Q_OBJECT

private slots:
    void everyPageHasACellWithItsNumberAsLabel() {
        Fixture f;
        QCOMPARE(f.view.pageCount(), 3U);
        QCOMPARE(f.view.labelFor(0), QStringLiteral("1"));
        QCOMPARE(f.view.labelFor(2), QStringLiteral("3"));
        // The page fits the thumbnail's width (page 0 is 220 x 140 points).
        QCOMPARE(f.view.imageRect(0).size(), QSize(120, 76));
        // The cells follow each other without gaps, as wide as the view.
        QCOMPARE(f.view.cellRect(0).width(), f.view.viewport()->width());
        QCOMPARE(f.view.cellRect(1).top(), f.view.cellRect(0).bottom() + 1);
        QCOMPARE(f.view.pageAt(f.centreOf(1)), 1U);
        QCOMPARE(f.view.pageAt(QPoint(5, f.view.viewport()->height() - 1)), 3U); // below the last
    }

    void labelsFromTheDocumentReplaceTheNumbers() {
        Fixture f;
        f.view.setPageLabels({QStringLiteral("i"), QString(), QStringLiteral("A-1")});
        QCOMPARE(f.view.labelFor(0), QStringLiteral("i"));
        QCOMPARE(f.view.labelFor(1), QStringLiteral("2")); // no label: the number
        QCOMPARE(f.view.labelFor(2), QStringLiteral("A-1"));
        QCOMPARE(f.view.labelFor(7), QStringLiteral("8"));
    }

    void thumbnailsOfTheVisiblePagesAreRenderedAndDrawn() {
        Fixture f;
        for (quint32 page = 0; page < 3; ++page) {
            QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(page), kWaitMs);
        }
        QVERIFY(f.view.requestsSent() >= 3);
        // What is drawn is not the placeholder.
        const QImage shown = f.view.viewport()->grab().toImage();
        const QRect image = f.view.imageRect(0);
        const QRgb placeholder = f.view.palette().color(QPalette::AlternateBase).rgb();
        int differing = 0;
        for (int y = image.top() + 2; y < image.bottom() - 2; ++y) {
            for (int x = image.left() + 2; x < image.right() - 2; ++x) {
                differing += shown.pixel(x, y) != placeholder ? 1 : 0;
            }
        }
        QVERIFY2(differing > 100, qPrintable(QString::number(differing)));
    }

    void aClickJumpsTheCanvasAndHighlightsThePage() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        QCOMPARE(f.controller.currentPage(), 0U);
        QTest::mouseClick(f.view.viewport(), Qt::LeftButton, {}, f.centreOf(2));
        QCOMPARE(f.view.highlightedPage(), 2U);
        QCOMPARE(f.controller.currentPage(), 2U);
        QVERIFY(f.view.isHighlighted(2));
        QVERIFY(!f.view.isHighlighted(1));
        QVERIFY(f.view.hasFocus());
    }

    void theHighlightFollowsTheCanvasAndStaysOnScreen() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        f.controller.goToPage(60);
        QCOMPARE(f.view.highlightedPage(), 60U);
        const ThumbnailView::Range seen = f.view.visiblePages();
        QVERIFY2(seen.first <= 60 && 60 < seen.end,
                 qPrintable(QStringLiteral("%1..%2").arg(seen.first).arg(seen.end)));
        // The cell is wholly in the view.
        const QRect cell = f.view.cellRect(60);
        QVERIFY(cell.top() >= 0 && cell.bottom() < f.view.viewport()->height());
    }

    void bothPagesOfASpreadAreHighlighted() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        f.controller.setSpread(vellora::PageLayout::Spread::Two);
        f.controller.goToPage(10);
        QVERIFY(f.view.isHighlighted(10));
        QVERIFY(f.view.isHighlighted(11));
        QVERIFY(!f.view.isHighlighted(9));
        QVERIFY(!f.view.isHighlighted(12));
    }

    void keysMoveThroughThePagesAndJumpTheCanvas() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        f.view.setFocus();
        QTest::keyClick(f.view.viewport(), Qt::Key_Down);
        QTest::keyClick(f.view.viewport(), Qt::Key_Down);
        QTest::keyClick(f.view.viewport(), Qt::Key_Down);
        QCOMPARE(f.view.selectedPage(), 3U);
        QCOMPARE(f.controller.currentPage(), 3U);
        QTest::keyClick(f.view.viewport(), Qt::Key_Up);
        QCOMPARE(f.controller.currentPage(), 2U);

        // Page Down moves by the cells on screen less one, so that the last one is the next first.
        const ThumbnailView::Range seen = f.view.visiblePages();
        const quint32 step = seen.end - seen.first - 1;
        QVERIFY(step >= 2);
        QTest::keyClick(f.view.viewport(), Qt::Key_PageDown);
        QCOMPARE(f.view.selectedPage(), 2U + step);
        QCOMPARE(f.controller.currentPage(), 2U + step);
        QTest::keyClick(f.view.viewport(), Qt::Key_PageUp);
        QCOMPARE(f.view.selectedPage(), 2U);

        QTest::keyClick(f.view.viewport(), Qt::Key_End);
        QCOMPARE(f.view.selectedPage(), 99U);
        QCOMPARE(f.controller.currentPage(), 99U);
        const QRect last = f.view.cellRect(99);
        QVERIFY(last.bottom() < f.view.viewport()->height());
        QTest::keyClick(f.view.viewport(), Qt::Key_Down); // nowhere further
        QCOMPARE(f.view.selectedPage(), 99U);
        QTest::keyClick(f.view.viewport(), Qt::Key_Home);
        QCOMPARE(f.view.selectedPage(), 0U);
        QCOMPARE(f.controller.currentPage(), 0U);
        QTest::keyClick(f.view.viewport(), Qt::Key_Up); // nowhere before
        QCOMPARE(f.view.selectedPage(), 0U);
    }

    void aKeyKeepsItsPageWhenTheCanvasReportsAnother() {
        // Zoomed out, several pages are on screen and the canvas's "current page" is the middle
        // one; the keyboard must go on from the page the user chose.
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        f.controller.setZoom(0.2, QPointF(0.0, 0.0));
        f.view.setFocus();
        QTest::keyClick(f.view.viewport(), Qt::Key_Down);
        QTest::keyClick(f.view.viewport(), Qt::Key_Down);
        QCOMPARE(f.view.selectedPage(), 2U);
        QCOMPARE(f.view.highlightedPage(), 2U);
    }

    void theThumbnailSizeIsClampedAndKeepsThePageAtTheTop() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 100));
        QSignalSpy changed(&f.view, &ThumbnailView::thumbnailWidthChanged);
        const double before = f.view.contentHeight();
        f.view.setThumbnailWidth(10'000);
        QCOMPARE(f.view.thumbnailWidth(), ThumbnailView::kMaxWidth);
        QVERIFY(f.view.contentHeight() > before * 1.9);
        f.view.setThumbnailWidth(1);
        QCOMPARE(f.view.thumbnailWidth(), ThumbnailView::kMinWidth);
        QCOMPARE(changed.size(), 2);
        f.view.setThumbnailWidth(ThumbnailView::kMinWidth); // no change, no signal
        QCOMPARE(changed.size(), 2);

        f.view.setThumbnailWidth(120);
        f.view.verticalScrollBar()->setValue(f.view.cellRect(40).top() +
                                             f.view.verticalScrollBar()->value());
        QCOMPARE(f.view.visiblePages().first, 40U);
        f.view.setThumbnailWidth(200);
        QCOMPARE(f.view.visiblePages().first, 40U);
        // Thumbnails are asked for at the new size and arrive.
        QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(40), kWaitMs);
    }

    void ctrlWheelResizesTheThumbnails() {
        Fixture f;
        QWheelEvent wheel(QPointF(10, 10), f.view.viewport()->mapToGlobal(QPointF(10, 10)),
                          QPoint(0, 0), QPoint(0, 120), Qt::NoButton, Qt::ControlModifier,
                          Qt::NoScrollPhase, false);
        QCoreApplication::sendEvent(f.view.viewport(), &wheel);
        QCOMPARE(f.view.thumbnailWidth(), ThumbnailView::kDefaultWidth + 8);
    }

    void aSidebarSliderSetsTheSize() {
        EngineSession session;
        CanvasController controller(&session);
        vellora::ThumbnailSidebar sidebar(&session, &controller);
        sidebar.resize(200, 400);
        sidebar.sizeSlider().setValue(200);
        QCOMPARE(sidebar.thumbnails().thumbnailWidth(), 200);
        sidebar.thumbnails().setThumbnailWidth(90);
        QCOMPARE(sidebar.sizeSlider().value(), 90);
    }

    void onlyWhatIsNearTheViewIsAskedForAndTheRestIsWithdrawn() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 10'000));
        QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(0), kWaitMs);
        QVERIFY2(f.view.requestsSent() <= ThumbnailView::kMaxWanted,
                 qPrintable(QString::number(f.view.requestsSent())));
        QVERIFY(f.view.thumbnailsInFlight() <= ThumbnailView::kMaxWanted);

        // A jump to the end: the thumbnails there are asked for and arrive; the ones for the
        // top of the document are not asked for again by the move.
        const quint64 before = f.view.requestsSent();
        f.view.goToPage(9'999);
        QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(9'999), kWaitMs);
        QVERIFY2(f.view.requestsSent() - before <= ThumbnailView::kMaxWanted,
                 qPrintable(QString::number(f.view.requestsSent() - before)));
        QTRY_VERIFY_WITH_TIMEOUT(f.view.thumbnailsInFlight() == 0, kWaitMs);
    }

    void aHiddenSidebarAsksForNothing() {
        EngineSession session;
        CanvasController controller(&session);
        ThumbnailView view(&session, &controller);
        view.resize(200, 500);
        QSignalSpy opened(&session, &EngineSession::opened);
        QVERIFY(session.open(QStringLiteral(VELLORA_GOLDEN_PDF)).isEmpty());
        QVERIFY(opened.wait(kWaitMs));
        QTest::qWait(300); // time enough for a request to have been sent
        QCOMPARE(view.requestsSent(), 0U);
        QCOMPARE(view.thumbnailsInFlight(), 0);

        view.show();
        QTRY_VERIFY_WITH_TIMEOUT(view.hasThumbnail(0), kWaitMs);
        QVERIFY(view.requestsSent() > 0);

        // Hidden again, it asks for nothing even when something changes that would make it ask.
        view.hide();
        QCOMPARE(view.thumbnailsInFlight(), 0);
        const quint64 sent = view.requestsSent();
        view.setThumbnailWidth(200);
        QTest::qWait(300);
        QCOMPARE(view.requestsSent(), sent);
        QCOMPARE(view.thumbnailsInFlight(), 0);
    }

    void aDocumentWithoutAnEngineHasNoCells() {
        EngineSession session;
        CanvasController controller(&session);
        ThumbnailView view(&session, &controller);
        view.resize(200, 400);
        view.show();
        QCOMPARE(view.pageCount(), 0U);
        QCOMPARE(view.visiblePages().end, view.visiblePages().first);
        QCOMPARE(view.pageAt(QPoint(5, 5)), 0U);
        QTest::keyClick(&view, Qt::Key_Down); // nothing to move through, and nothing breaks
        QTest::mouseClick(view.viewport(), Qt::LeftButton, {}, QPoint(5, 5));
        QVERIFY(!view.hasThumbnail(0));
    }

    void theSidebarIsHiddenUntilAskedForAndRemembersItsState() {
        QTemporaryDir dir;
        const QString ini = dir.filePath(QStringLiteral("s.ini"));
        {
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.show();
            QVERIFY(!window.currentTab().sidebarVisible());
            QVERIFY(window.commands().run(QStringLiteral("view.thumbnails")));
            QVERIFY(window.currentTab().sidebarVisible());
            window.currentTab().sidebar().thumbnails().setThumbnailWidth(180);
            QVERIFY(settings.sidebar().visible);
            QCOMPARE(settings.sidebar().thumbnailWidth, 180);
        }
        {
            // The next run opens its tabs the same way.
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.show();
            QVERIFY(window.currentTab().sidebarVisible());
            QCOMPARE(window.currentTab().sidebar().thumbnails().thumbnailWidth(), 180);
            QVERIFY(window.commands().run(QStringLiteral("view.thumbnails")));
            QVERIFY(!window.currentTab().sidebarVisible());
            QVERIFY(!settings.sidebar().visible);
        }
    }

    // Flinging through the list asks for nothing until it stops (the timing of this is measured in
    // `tst_thumbnails_frame_time`): 1,500 steps of a window and more make a few dozen requests, not
    // one for every page that went by, and the last screen is drawn once the list is still.
    void flingingThroughTenThousandThumbnailsAsksForNothingUntilItStops() {
        QTemporaryDir dir;
        Fixture f(writeSynthetic(dir, 10'000));
        QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(0), kWaitMs);
        const quint64 before = f.view.requestsSent();
        QScrollBar* bar = f.view.verticalScrollBar();
        int steps = 0;
        while (bar->value() < bar->maximum()) {
            bar->setValue(bar->value() + 1'200);
            ++steps;
            QCoreApplication::processEvents(); // the queued scheduling pass, as in real use
        }
        QVERIFY2(steps >= 1'400, qPrintable(QString::number(steps)));
        QCOMPARE(f.view.requestsSent(), before);
        QCOMPARE(f.view.thumbnailsInFlight(), 0);
        // Once the list has been still for a moment the last screen is asked for and drawn.
        QTRY_VERIFY_WITH_TIMEOUT(f.view.hasThumbnail(f.view.visiblePages().first), kWaitMs);
        QVERIFY(f.view.requestsSent() - before <= ThumbnailView::kMaxWanted);
    }
};

VELLORA_TEST_MAIN(TstThumbnails)
#include "tst_thumbnails.moc"
