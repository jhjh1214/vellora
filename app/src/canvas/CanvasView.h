// The scrolling view of a document: a scroll area whose viewport is the GPU canvas. It owns the
// controller, keeps the scroll bars in step with it, and turns Ctrl+wheel into zoom. Keys that
// scroll (arrows, Page Up/Down, Home/End) come from QAbstractScrollArea itself, except that Page
// Up/Down turn pages when the view shows one row at a time. Holding Z (or the "Zoom to Selection"
// command) and dragging a rectangle zooms to it.
#pragma once

#include "canvas/CanvasController.h"

#include <QAbstractScrollArea>

class QLabel;
class QRubberBand;

namespace vellora {

class CanvasWidget;

class CanvasView : public QAbstractScrollArea {
    Q_OBJECT

public:
    // Ctrl+wheel: one notch (120 units) multiplies the zoom by this much.
    static constexpr double kWheelZoomPerNotch = 1.1;
    // Ctrl+plus / Ctrl+minus multiply the zoom by this much.
    static constexpr double kZoomStep = 1.25;

    explicit CanvasView(EngineSession* session, QWidget* parent = nullptr);

    CanvasController* controller() { return &m_controller; }
    CanvasWidget* canvas() { return m_canvas; }

    // Forgets the old document's view; call before the session opens another file.
    void reset();

    // A non-modal notice over the top of the canvas (mouse clicks pass through it), for example
    // while the engine restarts. Plain text.
    void showBanner(const QString& text);
    void hideBanner();
    QString bannerText() const;
    bool bannerVisible() const;

    void zoomIn();
    void zoomOut();
    void actualSize();
    void fitWidth();
    void fitPage();

    // The rectangle tool: while armed, dragging with the left button draws a rectangle and zooms to
    // it. Held Z arms it for as long as the key is down; `armZoomRect(true)` arms it for one drag.
    void armZoomRect(bool armed);
    bool zoomRectArmed() const { return m_zoomRectOneShot || m_zKeyDown; }
    // The rectangle being dragged (viewport pixels); empty when none.
    QRect dragRect() const;

protected:
    bool viewportEvent(QEvent* event) override;
    void wheelEvent(QWheelEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void keyReleaseEvent(QKeyEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void focusOutEvent(QFocusEvent* event) override;

private:
    void syncScrollBars();
    void placeBanner();
    void updateCursor();
    void endDrag(bool zoom);

    CanvasController m_controller;
    CanvasWidget* m_canvas;
    QLabel* m_banner = nullptr;
    bool m_syncing = false;
    bool m_zKeyDown = false;
    bool m_zoomRectOneShot = false;
    QRubberBand* m_band = nullptr;
    QPoint m_dragStart;
    bool m_dragging = false;
};

} // namespace vellora
