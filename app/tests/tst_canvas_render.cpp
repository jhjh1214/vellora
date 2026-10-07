// The canvas drawing for real: a window on a platform that can run QRhi, a document, and the
// pixels of the framebuffer. Needs a GPU, a software rasteriser or Mesa (see CMakeLists.txt).
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "canvas/CanvasWidget.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

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
        window.canvas().controller()->setScrollPosition(QPointF(0.0, 1.0e12));
        QTRY_COMPARE_WITH_TIMEOUT(window.pageStatus(), QStringLiteral("Page 10000 / 10000"),
                                  kWaitMs);
        bool ink = false;
        QTRY_VERIFY_WITH_TIMEOUT(
            [&] {
                const QImage image = canvas->grabFramebuffer();
                const double ratio = static_cast<double>(image.width()) / canvas->width();
                const QRectF page = window.canvas().controller()->frame().pages.last().rect;
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
};

QTEST_MAIN(TstCanvasRender)
#include "tst_canvas_render.moc"
