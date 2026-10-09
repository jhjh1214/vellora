#include "text/SelectionOverlay.h"

#include <QPainter>
#include <QPalette>

namespace vellora {

SelectionOverlay::SelectionOverlay(CanvasController* controller, TextSelection* selection,
                                   QWidget* parent)
    : QWidget(parent), m_controller(controller), m_selection(selection) {
    setAttribute(Qt::WA_TransparentForMouseEvents);
    setAttribute(Qt::WA_NoSystemBackground);
    setAttribute(Qt::WA_TranslucentBackground);
    connect(m_selection, &TextSelection::changed, this, qOverload<>(&QWidget::update));
    connect(m_controller, &CanvasController::viewChanged, this, qOverload<>(&QWidget::update));
}

QList<QRectF> SelectionOverlay::highlights() const {
    QList<QRectF> out;
    if (m_selection->isEmpty()) {
        return out;
    }
    for (const PageDraw& page : m_controller->visiblePages()) {
        for (const QRectF& rect : m_selection->rectsOn(page.page)) {
            out.append(page.map(rect));
        }
    }
    return out;
}

void SelectionOverlay::paintEvent(QPaintEvent*) {
    const QList<QRectF> rects = highlights();
    if (rects.isEmpty()) {
        return;
    }
    QPainter painter(this);
    QColor color = palette().color(QPalette::Highlight);
    color.setAlpha(110);
    for (const QRectF& rect : rects) {
        painter.fillRect(rect, color);
    }
}

} // namespace vellora
