// The ring round the link that has the keyboard focus (or a screen reader's): a transparent widget
// above the canvas that never takes the mouse. The ring is two lines, black inside white, so that
// it shows on a white page and on a dark one whatever the system colours are (high contrast too).
#pragma once

#include <QRectF>
#include <QWidget>

namespace vellora {

class CanvasView;

class LinkFocusOverlay : public QWidget {
    Q_OBJECT

public:
    // `view` is not owned and must outlive the overlay.
    LinkFocusOverlay(CanvasView* view, QWidget* parent);

    // The ring's rectangle in viewport pixels, or an empty rectangle when no link is focused or it
    // is not on screen.
    QRectF ringRect() const;

protected:
    void paintEvent(QPaintEvent* event) override;

private:
    CanvasView* m_view;
};

} // namespace vellora
