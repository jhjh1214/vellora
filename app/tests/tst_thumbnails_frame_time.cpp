// The frame-time check of the thumbnail sidebar (M1 task 11): a fling through the thumbnails of a
// 10,000-page document, with the real window and the real engine. As for the canvas, none of our
// code may take longer than the 8 ms budget: not an event handler or timer (measured in
// `Application::notify`), and not one repaint of the sidebar.
//
// This is a timing test in a file of its own so that a platform whose CI runners cannot keep time
// can switch it off without losing the functional tests of `tst_thumbnails` (see CMakeLists.txt and
// "macOS and the canvas test" in app/README.md).
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "VelloraTestMain.h"
#include "diagnostics/Application.h"
#include "diagnostics/UiWatchdog.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QFile>
#include <QScrollBar>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTimer>

using vellora::EngineSession;
using vellora::ThumbnailView;

namespace {

constexpr int kWaitMs = 60'000;

} // namespace

class TstThumbnailsFrameTime : public QObject {
    Q_OBJECT

private slots:
    void flingingThroughTenThousandThumbnailsStaysWithinTheFrameBudget() {
        QTemporaryDir dir;
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        vellora::MainWindow window;
        window.resize(900, 700);
        window.show();
        QSignalSpy opened(&window.session(), &EngineSession::opened);
        QVERIFY(window.openDocument(path));
        QVERIFY(opened.wait(kWaitMs));
        window.currentTab().setSidebarVisible(true);
        ThumbnailView& view = window.currentTab().sidebar().thumbnails();
        QCOMPARE(view.pageCount(), 10'000U);
        QTRY_VERIFY_WITH_TIMEOUT(view.hasThumbnail(0), kWaitMs);

        vellora::UiWatchdog watchdog;
        auto* application = qobject_cast<vellora::Application*>(QCoreApplication::instance());
        QVERIFY(application != nullptr);
        application->setWatchdog(&watchdog);
        connect(&view, &ThumbnailView::paintTimed, &watchdog,
                [&watchdog](double ms) { watchdog.recordFrame(ms); });

        // Every 8 ms the list moves by about six cells, until the end: 1,500 steps and more.
        QScrollBar* bar = view.verticalScrollBar();
        int steps = 0;
        QTimer fling;
        fling.setTimerType(Qt::PreciseTimer);
        fling.setInterval(8);
        bool done = false;
        connect(&fling, &QTimer::timeout, this, [&] {
            ++steps;
            bar->setValue(bar->value() + 1'200);
            done = bar->value() >= bar->maximum();
        });
        watchdog.start();
        fling.start();
        QTRY_VERIFY_WITH_TIMEOUT(done, 120'000);
        fling.stop();
        // Let the list settle and the engine answer, so that the measurement includes the tail: the
        // thumbnails of the last screen are asked for once the list is still, and drawn.
        QTRY_VERIFY_WITH_TIMEOUT(view.hasThumbnail(view.visiblePages().first), kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(view.thumbnailsInFlight() == 0, kWaitMs);
        watchdog.stop();
        application->setWatchdog(nullptr);

        qInfo("fling: %d steps; %s; %llu requests sent", steps, qPrintable(watchdog.summary()),
              static_cast<unsigned long long>(view.requestsSent()));
        QVERIFY2(steps >= 1'400, qPrintable(QString::number(steps)));
        // The measurements saw real work: repaints and handlers.
        QVERIFY2(watchdog.frames().samples >= 100, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.handlers().samples >= 1'400, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.frames().overBudget == 0, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.handlers().overBudget == 0, qPrintable(watchdog.summary()));
    }
};

VELLORA_TEST_MAIN(TstThumbnailsFrameTime)
#include "tst_thumbnails_frame_time.moc"
