// The scrolling view of a document: a scroll area whose viewport is the GPU canvas. It owns the
// controller, keeps the scroll bars in step with it, and turns Ctrl+wheel into zoom. Keys that
// scroll (arrows, Page Up/Down, Home/End) come from QAbstractScrollArea itself.
#pragma once

#include "canvas/CanvasController.h"

#include <QAbstractScrollArea>

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

    void zoomIn();
    void zoomOut();
    void actualSize();
    void fitWidth();

protected:
    bool viewportEvent(QEvent* event) override;
    void wheelEvent(QWheelEvent* event) override;

private:
    void syncScrollBars();

    CanvasController m_controller;
    CanvasWidget* m_canvas;
    bool m_syncing = false;
};

} // namespace vellora
