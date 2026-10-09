// CanvasView and MainWindow with the canvas: scroll bars, wheel zoom and the status bar. Runs on
// the offscreen platform, where QRhiWidget cannot render (tst_canvas_render covers drawing).
#include "MainWindow.h"

#include <QScrollBar>
#include <QSignalSpy>
#include <QTest>
#include <QWheelEvent>

namespace {

constexpr int kWaitMs = 60'000;

void sendWheel(QWidget* target, QPoint position, int delta, Qt::KeyboardModifiers modifiers) {
    QWheelEvent event(QPointF(position), QPointF(target->mapToGlobal(position)), QPoint(),
                      QPoint(0, delta), Qt::NoButton, modifiers, Qt::NoScrollPhase, false);
    QApplication::sendEvent(target, &event);
}

} // namespace

class TstCanvasView : public QObject {
    Q_OBJECT

private slots:
    void scrollBarsFollowTheController() {
        vellora::EngineSession session;
        vellora::CanvasView view(&session);
        view.resize(500, 120);
        view.show();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QVERIFY(session.open(QStringLiteral(VELLORA_GOLDEN_PDF)).isEmpty());
        QVERIFY(opened.wait(kWaitMs));

        QScrollBar* bar = view.verticalScrollBar();
        const int viewHeight = view.viewport()->height();
        QCOMPARE(bar->minimum(), 0);
        QVERIFY(bar->maximum() > 0);
        const QSizeF content = view.controller()->contentSize();
        QCOMPARE(static_cast<double>(bar->maximum() + viewHeight), content.height());

        // Moving the bar moves the view, and the other way round.
        bar->setValue(100);
        QCOMPARE(view.controller()->scrollPosition().y(), 100.0);
        view.controller()->setScrollPosition(QPointF(0.0, 40.0));
        QCOMPARE(bar->value(), 40);
    }

    void ctrlWheelZoomsAndPlainWheelScrolls() {
        vellora::EngineSession session;
        vellora::CanvasView view(&session);
        view.resize(500, 300);
        view.show();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QVERIFY(session.open(QStringLiteral(VELLORA_GOLDEN_PDF)).isEmpty());
        QVERIFY(opened.wait(kWaitMs));

        const double before = view.controller()->zoom();
        sendWheel(view.viewport(), QPoint(100, 100), 120, Qt::ControlModifier);
        QVERIFY(view.controller()->zoom() > before);
        QCOMPARE(view.controller()->zoom(), before * vellora::CanvasView::kWheelZoomPerNotch);
        sendWheel(view.viewport(), QPoint(100, 100), -120, Qt::ControlModifier);
        QVERIFY(qAbs(view.controller()->zoom() - before) < 1e-9);

        const double scrollBefore = view.controller()->scrollPosition().y();
        sendWheel(view.viewport(), QPoint(100, 100), -120, Qt::NoModifier);
        QVERIFY(view.controller()->scrollPosition().y() > scrollBefore);
    }

    void zoomCommandsMoveInSteps() {
        vellora::EngineSession session;
        vellora::CanvasView view(&session);
        view.resize(500, 300);
        view.show();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QVERIFY(session.open(QStringLiteral(VELLORA_GOLDEN_PDF)).isEmpty());
        QVERIFY(opened.wait(kWaitMs));

        view.zoomIn();
        QCOMPARE(view.controller()->zoom(), vellora::CanvasView::kZoomStep);
        view.zoomOut();
        QVERIFY(qAbs(view.controller()->zoom() - 1.0) < 1e-9);
        view.fitWidth();
        QVERIFY(view.controller()->zoom() > 1.0);
        view.actualSize();
        QCOMPARE(view.controller()->zoom(), 1.0);
    }

    void theWindowShowsPageAndZoom() {
        vellora::MainWindow window;
        window.resize(500, 250); // short, so that the middle of the window is still on page 1
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(window.zoomStatus(), QStringLiteral("100%"));
        QTRY_COMPARE(window.pageStatus(), QStringLiteral("Page 1 / 3"));

        window.canvas().zoomIn();
        QCOMPARE(window.zoomStatus(), QStringLiteral("125%"));
        window.canvas().controller()->setScrollPosition(QPointF(0.0, 1.0e9));
        QCOMPARE(window.pageStatus(), QStringLiteral("Page 3 / 3"));
    }

    void openingAnotherDocumentResetsTheView() {
        vellora::MainWindow window;
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        window.canvas().zoomIn();
        window.canvas().controller()->setScrollPosition(QPointF(0.0, 1.0e9));

        // Replacing the document of the tab (opening the same file elsewhere would only switch to
        // its tab).
        QVERIFY(window.currentTab().open(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QCOMPARE(window.canvas().controller()->zoom(), 1.0);
        QCOMPARE(window.canvas().controller()->scrollPosition(), QPointF(0.0, 0.0));
        QCOMPARE(window.zoomStatus(), QStringLiteral("100%"));
    }
};

QTEST_MAIN(TstCanvasView)
#include "tst_canvas_view.moc"
