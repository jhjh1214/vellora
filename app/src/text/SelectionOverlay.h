// Paints the selected text: a translucent rectangle over each line of it. A transparent widget
// above the GPU canvas that never takes the mouse, so the canvas needs no knowledge of text.
#pragma once

#include "canvas/CanvasController.h"
#include "text/TextSelection.h"

#include <QWidget>

namespace vellora {

class CaretBrowser;

class SelectionOverlay : public QWidget {
    Q_OBJECT

public:
    // Neither is owned; both must outlive the overlay.
    SelectionOverlay(CanvasController* controller, TextSelection* selection,
                     QWidget* parent = nullptr);

    // The rectangles painted, in viewport pixels (for tests and for the accessibility tree).
    QList<QRectF> highlights() const;
    // The caret to paint while caret browsing is on (not owned); null for none.
    void setCaretBrowser(CaretBrowser* caret);
    // The caret as painted, in viewport pixels; empty when there is none on screen.
    QRectF caretRect() const;

protected:
    void paintEvent(QPaintEvent* event) override;

private:
    CanvasController* m_controller;
    TextSelection* m_selection;
    CaretBrowser* m_caret = nullptr;
};

} // namespace vellora
