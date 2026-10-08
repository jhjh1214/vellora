// The canvas drawing for real: a window on a platform that can run QRhi, a document, and the
// pixels of the framebuffer. Needs a GPU, a software rasteriser or Mesa (see CMakeLists.txt).
#include "KillProcess.h"
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "canvas/CanvasWidget.h"
#include "diagnostics/UiWatchdog.h"

#include <QEventLoop>
#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTimer>

namespace {

constexpr int kWaitMs = 60'000;

bool isGrey(QRgb pixel) {
    return qAbs(qRed(pixel) - 0xC8) < 4 && qAbs(qGreen(pixel) - 0xC8) < 4 &&
           qAbs(qBlue(pixel) - 0xC8) < 4;
}

} // namespace

class TstCanvasRender : public QObject {
    Q_OBJECT

private slots:
    void tilesAreDrawnOnTheirPages() {
        vellora::MainWindow window;
        window.resize(700, 500);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));

        vellora::CanvasWidget* canvas = window.canvas().canvas();
        // Tiles arrive, the canvas repaints, and finished tiles become textures.
        QTRY_VERIFY_WITH_TIMEOUT(canvas->framesRendered() > 0, kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

        // Wait until a frame after the first tile was uploaded has been drawn.
        const quint64 frames = canvas->framesRendered();
        window.canvas().controller()->setScrollPosition(QPointF(0.0, 1.0));
        window.canvas().controller()->setScrollPosition(QPointF(0.0, 0.0));
        QTRY_VERIFY_WITH_TIMEOUT(canvas->framesRendered() > frames, kWaitMs);

        const QImage image = canvas->grabFramebuffer();
        QVERIFY(!image.isNull());
        const double ratio = static_cast<double>(image.width()) / canvas->width();

        // The first page is 220 x 140 points, centred, 12 pixels from the top.
        const vellora::Frame frame = window.canvas().controller()->frame();
        QVERIFY(!frame.pages.isEmpty());
        const QRectF page = frame.pages.first().rect;
        const auto pixelAt = [&](double x, double y) {
            return image.pixel(static_cast<int>(x * ratio), static_cast<int>(y * ratio));
        };
        // Outside the page is the grey background...
        QVERIFY2(isGrey(pixelAt(2, 2)), qPrintable(QString::number(pixelAt(2, 2), 16)));
        QVERIFY(isGrey(pixelAt(page.right() + 20, page.center().y())));
        // ...inside it the page is white, and the tile put something on it.
        bool white = false;
        bool ink = false;
        for (int y = 0; y < page.height(); ++y) {
            for (int x = 0; x < page.width(); ++x) {
                const QRgb pixel = pixelAt(page.left() + x, page.top() + y);
                white = white || pixel == qRgb(255, 255, 255);
                ink = ink || (!isGrey(pixel) && pixel != qRgb(255, 255, 255));
            }
        }
        QVERIFY(white);
        QVERIFY2(ink, "the page is blank: the tile was not drawn");
    }

    void theEndOfATenThousandPageDocumentIsDrawn() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        vellora::MainWindow window;
        window.resize(700, 500);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(path));
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(window.documentStatus(), QStringLiteral("10000 page(s)"));

        vellora::CanvasWidget* canvas = window.canvas().canvas();
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

        // The last page: the status bar says so, and its tiles are drawn on screen.
        // Scrolled so that the top of the last page, where its title is, is on screen.
        vellora::CanvasController* controller = window.canvas().controller();
        controller->setScrollPosition(
            QPointF(0.0, controller->layout().pageTop(9'999, controller->zoom()) -
                             vellora::PageLayout::kGap));
        QTRY_COMPARE_WITH_TIMEOUT(window.pageStatus(), QStringLiteral("Page 10000 / 10000"),
                                  kWaitMs);
        bool ink = false;
        QTRY_VERIFY_WITH_TIMEOUT(
            [&] {
                const QImage image = canvas->grabFramebuffer();
                const double ratio = static_cast<double>(image.width()) / canvas->width();
                // The part of the last page that is on screen.
                const QRectF page =
                    window.canvas().controller()->frame().pages.last().rect.intersected(
                        QRectF(QPointF(0, 0), canvas->size()));
                for (int y = 0; y < page.height() && !ink; ++y) {
                    for (int x = 0; x < page.width() && !ink; ++x) {
                        const QRgb pixel = image.pixel(static_cast<int>((page.left() + x) * ratio),
                                                       static_cast<int>((page.top() + y) * ratio));
                        ink = qRed(pixel) < 64 && qGreen(pixel) < 64 && qBlue(pixel) < 64;
                    }
                }
                return ink;
            }(),
            kWaitMs);
    }

    // Scripted scrolling and zooming through a 10,000-page document with the stall watchdog
    // running: the numbers go to the log (the acceptance of M0 task 22 reads them on a developer
    // machine with a real GPU). Only a catastrophic stall fails the test, because CI renders in
    // software, which legitimately stalls the UI thread.
    void scrollingATenThousandPageDocumentIsWatched() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        vellora::MainWindow window;
        window.resize(900, 700);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(path));
        QVERIFY(opened.wait(kWaitMs));
        vellora::CanvasWidget* canvas = window.canvas().canvas();
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

        vellora::UiWatchdog watchdog;
        watchdog.start();
        vellora::CanvasController* controller = window.canvas().controller();
        const quint64 framesBefore = canvas->framesRendered();
        // Driven by a timer inside the event loop, like input events, never by sleeping: a sleep
        // would itself block the loop and show up as a stall.
        QElapsedTimer clock;
        clock.start();
        double y = 0.0;
        int steps = 0;
        QEventLoop loop;
        QTimer driver;
        driver.setTimerType(Qt::PreciseTimer);
        driver.setInterval(8);
        connect(&driver, &QTimer::timeout, &loop, [&] {
            if (clock.elapsed() >= 4000) {
                loop.quit();
                return;
            }
            y += 350.0; // about a page every two steps
            controller->setScrollPosition(QPointF(0.0, y));
            if (steps % 100 == 50) {
                controller->zoomBy(1.25, QPointF(300.0, 300.0)); // a new zoom bucket now and then
            } else if (steps % 100 == 99) {
                controller->zoomBy(0.8, QPointF(300.0, 300.0));
            }
            ++steps;
        });
        driver.start();
        loop.exec();
        watchdog.stop();
        qInfo("render() in the last frame %.2f ms (tile upload %.2f ms), slowest %.2f ms",
              canvas->lastRenderMs(), canvas->lastUploadMs(), canvas->slowestRenderMs());
        qInfo("scrolled %d steps over page %u of 10000, %llu frames, %d textures; "
              "UI stalls over %lld ms: %d, longest %lld ms",
              steps, controller->currentPage() + 1,
              static_cast<unsigned long long>(canvas->framesRendered() - framesBefore),
              canvas->textureCount(), static_cast<long long>(vellora::UiWatchdog::kStallMs),
              watchdog.stallCount(), static_cast<long long>(watchdog.longestStallMs()));
        // How far the scripted scroll got depends on how fast frames present (vsync), so only
        // that it made real progress is asserted.
        QVERIFY(steps >= 20);
        QVERIFY(controller->currentPage() >= 5);
        QVERIFY2(watchdog.longestStallMs() < 500,
                 qPrintable(QString::number(watchdog.longestStallMs())));
        // Nothing is left asking the engine for tiles that are no longer wanted.
        QTRY_VERIFY_WITH_TIMEOUT(controller->tilesInFlight() <= 64, kWaitMs);
    }

    void drawingContinuesAfterTheEngineIsKilled() {
        vellora::MainWindow window;
        window.resize(700, 500);
        window.show();
        vellora::EngineSession& session = window.session();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QSignalSpy crashed(&session, &vellora::EngineSession::engineCrashed);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        vellora::CanvasWidget* canvas = window.canvas().canvas();
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

        killProcess(session.engineProcessId());
        QTRY_VERIFY_WITH_TIMEOUT(crashed.size() >= 1, kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(opened.size() >= 2, kWaitMs); // the new engine has the document
        QVERIFY(!window.canvas().bannerVisible());

        // A zoom the dead engine never drew: new tiles are rendered by the new engine and drawn.
        const int textures = canvas->textureCount();
        window.canvas().controller()->setZoom(2.0, QPointF(0.0, 0.0));
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > textures, kWaitMs);
    }
};

QTEST_MAIN(TstCanvasRender)
#include "tst_canvas_render.moc"
