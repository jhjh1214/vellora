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
#include "search/SearchController.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTransform>
#include <utility>

namespace {

constexpr int kWaitMs = 60'000;
// The scripted session takes at least 2,000 steps of 8 ms, more when every frame waits for a vsync.
constexpr int kScriptWaitMs = 300'000;

bool isGrey(QRgb pixel) {
    return qAbs(qRed(pixel) - 0xC8) < 4 && qAbs(qGreen(pixel) - 0xC8) < 4 &&
           qAbs(qBlue(pixel) - 0xC8) < 4;
}

// The framebuffer once every visible tile of `window`'s canvas is on the screen: the engine has
// rendered them, and a few frames have uploaded and drawn them.
QImage settledFrame(vellora::MainWindow& window) {
    vellora::CanvasController* controller = window.canvas().controller();
    vellora::CanvasWidget* canvas = window.canvas().canvas();
    QByteArray scratch;
    for (const vellora::TileDraw& tile : controller->frame().tiles) {
        const bool ready = QTest::qWaitFor(
            [&] {
                return window.session().readTile(tile.page, tile.scale, tile.x, tile.y, scratch);
            },
            kWaitMs);
        if (!ready) {
            return {};
        }
    }
    QImage image;
    for (int i = 0; i < 3; ++i) {
        image = canvas->grabFramebuffer();
    }
    return image;
}

// The part of `image` that shows `rect` (logical pixels of the canvas).
QImage cropped(const QImage& image, const vellora::CanvasWidget* canvas, const QRectF& rect) {
    const double ratio = static_cast<double>(image.width()) / canvas->width();
    return image.copy(QRect(QPoint(qRound(rect.left() * ratio), qRound(rect.top() * ratio)),
                            QSize(qRound(rect.width() * ratio), qRound(rect.height() * ratio))));
}

// How far two images of the same size are apart: the mean absolute difference of the channels, and
// the share of pixels where any channel differs by more than 48 levels.
std::pair<double, double> difference(const QImage& a, const QImage& b) {
    if (a.size() != b.size() || a.isNull()) {
        return {255.0, 1.0};
    }
    const QImage x = a.convertToFormat(QImage::Format_RGB32);
    const QImage y = b.convertToFormat(QImage::Format_RGB32);
    double sum = 0.0;
    long long loud = 0;
    for (int row = 0; row < x.height(); ++row) {
        for (int col = 0; col < x.width(); ++col) {
            const QRgb p = x.pixel(col, row);
            const QRgb q = y.pixel(col, row);
            const int dr = qAbs(qRed(p) - qRed(q));
            const int dg = qAbs(qGreen(p) - qGreen(q));
            const int db = qAbs(qBlue(p) - qBlue(q));
            sum += dr + dg + db;
            loud += (dr > 48 || dg > 48 || db > 48) ? 1 : 0;
        }
    }
    const double pixels = static_cast<double>(x.width()) * x.height();
    return {sum / (3.0 * pixels), static_cast<double>(loud) / pixels};
}

// `image` turned `turns` quarter turns clockwise, pixel for pixel (QImage::transformed samples).
QImage turnedClockwise(const QImage& image, int turns) {
    QImage current = image.convertToFormat(QImage::Format_RGB32);
    for (int i = 0; i < ((turns % 4) + 4) % 4; ++i) {
        QImage next(current.height(), current.width(), QImage::Format_RGB32);
        for (int y = 0; y < next.height(); ++y) {
            for (int x = 0; x < next.width(); ++x) {
                next.setPixel(x, y, current.pixel(y, current.height() - 1 - x));
            }
        }
        current = next;
    }
    return current;
}

// How many pixels are neither the page's white nor the background grey: ink.
long long inkPixels(const QImage& image) {
    long long ink = 0;
    for (int row = 0; row < image.height(); ++row) {
        for (int col = 0; col < image.width(); ++col) {
            const QRgb pixel = image.pixel(col, row);
            ink += (!isGrey(pixel) && pixel != qRgb(255, 255, 255)) ? 1 : 0;
        }
    }
    return ink;
}
} // namespace

namespace {

// What one scripted session measured.
struct SessionResult {
    QString summary;
    quint64 overBudget = 0;
};

// One scripted session on a window of its own, so that every attempt starts with a cold tile
// cache. Everything but the budget is checked here and fails the test outright.
void runScriptedSession(const QString& path, bool continuous, int spread, int rotation,
                        bool searching, SessionResult& result) {
    vellora::MainWindow window;
    window.resize(900, 700);
    window.show();
    QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
    QVERIFY(window.openDocument(path));
    QVERIFY(opened.wait(kWaitMs));
    vellora::CanvasWidget* canvas = window.canvas().canvas();
    window.canvas().controller()->setViewMode(
        {continuous, static_cast<vellora::PageLayout::Spread>(spread), rotation});
    QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

    vellora::UiWatchdog watchdog;
    auto* application = qobject_cast<vellora::Application*>(QCoreApplication::instance());
    QVERIFY(application != nullptr);
    application->setWatchdog(&watchdog);
    watchdog.watch(canvas);
    vellora::CanvasController* controller = window.canvas().controller();
    vellora::DiagnosticsScript script(controller);
    QSignalSpy finished(&script, &vellora::DiagnosticsScript::finished);
    if (searching) {
        // The engine searches all 10,000 pages and streams a thousand hits while the session
        // scrolls and zooms (the pages say "Page 1" ... "Page 10000").
        window.currentTab().search().start({QStringLiteral("Page 1"), false, false, false});
    }
    const quint64 framesBefore = canvas->framesRendered();
    watchdog.start();
    script.start();
    const bool done = finished.wait(kScriptWaitMs);
    watchdog.stop();
    application->setWatchdog(nullptr);

    result.summary = watchdog.summary();
    result.overBudget = watchdog.frames().overBudget + watchdog.handlers().overBudget;
    qInfo("%s: %s; %llu frames drawn, %d textures, ended on page %u of 10000",
          QTest::currentDataTag(), qPrintable(result.summary),
          static_cast<unsigned long long>(canvas->framesRendered() - framesBefore),
          canvas->textureCount(), controller->currentPage() + 1);
    QVERIFY2(done, "the scripted session did not finish");
    QCOMPARE(script.stepsDone(), 2000);
    QCOMPARE(script.zoomsDone(), 20);
    // The measurements saw real work: frames were drawn and handlers ran.
    QVERIFY2(watchdog.frames().samples >= 20, qPrintable(result.summary));
    QVERIFY2(watchdog.handlers().samples >= 2000, qPrintable(result.summary));
    // Nothing is left asking the engine for tiles that are no longer wanted.
    QTRY_VERIFY_WITH_TIMEOUT(controller->tilesInFlight() <= 64, kWaitMs);
    if (searching) {
        // It found them all: page 1, 10-19, 100-199, 1000-1999 and 10000.
        vellora::SearchController& search = window.currentTab().search();
        QTRY_VERIFY_WITH_TIMEOUT(!search.isRunning(), kWaitMs);
        QCOMPARE(search.state(), vellora::SearchController::State::Finished);
        QCOMPARE(search.count(), 1 + 10 + 100 + 1000 + 1);
    }
}

// Runs the scripted session on up to `kAttempts` cold sessions and passes when one stays within
// the frame budget (see the comment on the test that uses it).
void judgeWithinBudget(const QString& path, bool continuous, int spread, int rotation,
                       bool searching) {
    constexpr int kAttempts = 3;
    QStringList attempts;
    for (int attempt = 1; attempt <= kAttempts; ++attempt) {
        SessionResult result;
        runScriptedSession(path, continuous, spread, rotation, searching, result);
        if (QTest::currentTestFailed()) {
            return;
        }
        if (result.overBudget == 0) {
            if (attempt > 1) {
                qWarning("%s: within budget on attempt %d after: %s", QTest::currentDataTag(),
                         attempt, qPrintable(attempts.join(QStringLiteral(" | "))));
            }
            return;
        }
        attempts << QStringLiteral("attempt %1: %2").arg(attempt).arg(result.summary);
    }
    QFAIL(qPrintable(QStringLiteral("over budget on all %1 attempts: %2")
                         .arg(kAttempts)
                         .arg(attempts.join(QStringLiteral(" | ")))));
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

    // ---- M1 task 10: a turned view, and zoom without white flashes ----

    void aTurnedViewIsThePageTurned() {
        vellora::MainWindow window;
        window.resize(700, 500);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        vellora::CanvasWidget* canvas = window.canvas().canvas();
        vellora::CanvasController* controller = window.canvas().controller();
        QTRY_VERIFY_WITH_TIMEOUT(canvas->textureCount() > 0, kWaitMs);

        const QImage upright = settledFrame(window);
        QVERIFY(!upright.isNull());
        const QRectF uprightPage = controller->frame().pages.first().rect;
        const QImage reference = cropped(upright, canvas, uprightPage);
        QVERIFY2(inkPixels(reference) > 100, "nothing is drawn on the page to compare");

        for (const int turns : {1, 2, 3}) {
            controller->setRotation(turns);
            // The page that was in the middle of the window stays current; compare page 1.
            controller->goToPage(0);
            const QImage turned = settledFrame(window);
            QVERIFY(!turned.isNull());
            const QRectF page = controller->frame().pages.first().rect;
            QVERIFY2(page.top() >= 0.0,
                     qPrintable(QStringLiteral("turns %1 page %2,%3 %4x%5 scroll %6,%7 zoom %8")
                                    .arg(turns)
                                    .arg(page.left())
                                    .arg(page.top())
                                    .arg(page.width())
                                    .arg(page.height())
                                    .arg(controller->scrollPosition().x())
                                    .arg(controller->scrollPosition().y())
                                    .arg(controller->zoom())));
            // The page as shown is the upright page turned `turns` quarters clockwise.
            const QImage expected = turnedClockwise(reference, turns);
            const auto [mean, loud] = difference(cropped(turned, canvas, page), expected);
            // The page is drawn at the screen's scale, not 1:1 (the device pixel ratio), so the
            // turned and the upright texture are resampled at different sub-pixel offsets: thin
            // lines differ at their edges. That is a few percent of the pixels; a wrong turn, a
            // mirror or a missing turn differs in most of the page.
            const auto [wrongMean, wrongLoud] =
                difference(cropped(turned, canvas, page), turnedClockwise(reference, turns + 2));
            QVERIFY2(mean < 4.0 && loud < 0.05,
                     qPrintable(QStringLiteral("%1 quarter turns: mean difference %2, %3 of the "
                                               "pixels are far off")
                                    .arg(turns)
                                    .arg(mean)
                                    .arg(loud)));
            // The same comparison against the page turned two quarters further must fail.
            // (Measured: about 2 for the right turn and about 17 for a wrong one.)
            Q_UNUSED(wrongLoud);
            QVERIFY2(wrongMean > 8.0,
                     qPrintable(QStringLiteral("a wrong turn is not told apart: mean %1 against %2")
                                    .arg(wrongMean)
                                    .arg(mean)));
        }
    }

    void zoomingNeverShowsAWhitePageWhileSharpTilesLoad() {
        for (const int turns : {0, 1}) {
            vellora::MainWindow window;
            window.resize(700, 500);
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
            QVERIFY(opened.wait(kWaitMs));
            vellora::CanvasWidget* canvas = window.canvas().canvas();
            vellora::CanvasController* controller = window.canvas().controller();
            controller->setRotation(turns);
            QVERIFY(!settledFrame(window).isNull());

            // Zoom, and draw at once: the event loop has not run, so no tile of the new scale can
            // have arrived. What is on the page must still be the old tiles, scaled up.
            controller->setZoom(2.0, QPointF(350.0, 100.0));
            const QImage stand = canvas->grabFramebuffer();
            QVERIFY(!stand.isNull());
            const QRectF page = controller->frame().pages.first().rect;
            const QImage standIn = cropped(stand, canvas, page.intersected(QRectF(canvas->rect())));
            const long long standInInk = inkPixels(standIn);

            // The sharp version of the same view.
            const QImage sharp = settledFrame(window);
            QVERIFY(!sharp.isNull());
            const long long sharpInk =
                inkPixels(cropped(sharp, canvas, page.intersected(QRectF(canvas->rect()))));
            QVERIFY2(sharpInk > 100, "nothing is drawn on the page");
            // The stand-in is blurry but it is the page: it carries a large part of the ink, and
            // is not far from the sharp picture overall.
            QVERIFY2(standInInk > sharpInk / 2,
                     qPrintable(QStringLiteral("turn %1: the page is blank while loading (%2 ink "
                                               "pixels against %3)")
                                    .arg(turns)
                                    .arg(standInInk)
                                    .arg(sharpInk)));
            const auto [mean, loud] = difference(
                standIn, cropped(sharp, canvas, page.intersected(QRectF(canvas->rect()))));
            QVERIFY2(mean < 25.0, qPrintable(QStringLiteral("mean difference %1").arg(mean)));
            Q_UNUSED(loud);
        }
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
    // ours and is not measured, which is why this holds on CI's software rasterisers. It holds in
    // every view mode (M1 task 10): continuous or a page at a time, one or two pages a row, turned.
    void scrollingATenThousandPageDocumentStaysWithinTheFrameBudget_data() {
        QTest::addColumn<bool>("continuous");
        QTest::addColumn<int>("spread");
        QTest::addColumn<int>("rotation");
        QTest::newRow("continuous") << true << 0 << 0;
        QTest::newRow("single page") << false << 0 << 0;
        QTest::newRow("two pages continuous") << true << 1 << 0;
        QTest::newRow("two pages with a cover, a spread at a time") << false << 2 << 0;
        QTest::newRow("continuous, turned") << true << 0 << 1;
    }

    // The budget is wall-clock time of our own code, and the CI runners are shared machines: the
    // same session has been measured with a 334 ms stall of the main thread in one run and none
    // in the next, in a different mode each time, with the code unchanged. So the budget (8 ms,
    // never loosened) is judged on up to `kAttempts` sessions, each from a cold start; the test
    // passes when one of them stays within it. Code that is over budget does so every time, so a
    // regression fails all attempts. Any other failure (the script not finishing, wrong counts)
    // is not retried.
    void scrollingATenThousandPageDocumentStaysWithinTheFrameBudget() {
        QFETCH(bool, continuous);
        QFETCH(int, spread);
        QFETCH(int, rotation);
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        judgeWithinBudget(path, continuous, spread, rotation, false);
    }

    // The same, while the engine searches the whole document and streams its hits (M1 task 15).
    void scrollingWhileSearchingStaysWithinTheFrameBudget() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();
        judgeWithinBudget(path, true, 0, 0, true);
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
