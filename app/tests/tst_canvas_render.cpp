// The canvas drawing for real: a window on a platform that can run QRhi, a document, and the
// pixels of the framebuffer. Needs a GPU, a software rasteriser or Mesa (see CMakeLists.txt).
#include "KillProcess.h"
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasWidget.h"
#include "diagnostics/Application.h"
#include "diagnostics/DiagnosticsScript.h"
#include "diagnostics/UiWatchdog.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

namespace {

constexpr int kWaitMs = 60'000;
// The scripted session takes at least 2,000 steps of 8 ms, more when every frame waits for a vsync.
constexpr int kScriptWaitMs = 300'000;

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

    // The frame-time check (M1 task 1): the scripted session of `--diagnostics-script` scrolls
    // 2,000 pages of a 10,000-page document and zooms in and out 20 times, with the real canvas
    // drawing frames. None of our code may take longer than its budget: not an event handler or
    // timer (measured in `Application::notify`), not one `render()`. The vsynced present is not
    // ours and is not measured, which is why this holds on CI's software rasterisers.
    void scrollingATenThousandPageDocumentStaysWithinTheFrameBudget() {
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
        auto* application = qobject_cast<vellora::Application*>(QCoreApplication::instance());
        QVERIFY(application != nullptr);
        application->setWatchdog(&watchdog);
        watchdog.watch(canvas);
        vellora::CanvasController* controller = window.canvas().controller();
        vellora::DiagnosticsScript script(controller);
        QSignalSpy finished(&script, &vellora::DiagnosticsScript::finished);
        const quint64 framesBefore = canvas->framesRendered();
        watchdog.start();
        script.start();
        const bool done = finished.wait(kScriptWaitMs);
        watchdog.stop();
        application->setWatchdog(nullptr);

        qInfo("%s; %llu frames drawn, %d textures, ended on page %u of 10000",
              qPrintable(watchdog.summary()),
              static_cast<unsigned long long>(canvas->framesRendered() - framesBefore),
              canvas->textureCount(), controller->currentPage() + 1);
        QVERIFY2(done, "the scripted session did not finish");
        QCOMPARE(script.stepsDone(), 2000);
        QCOMPARE(script.zoomsDone(), 20);
        // The measurements saw real work: frames were drawn and handlers ran.
        QVERIFY2(watchdog.frames().samples >= 20, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.handlers().samples >= 2000, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.frames().overBudget == 0, qPrintable(watchdog.summary()));
        QVERIFY2(watchdog.handlers().overBudget == 0, qPrintable(watchdog.summary()));
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

VELLORA_TEST_MAIN(TstCanvasRender)
#include "tst_canvas_render.moc"
