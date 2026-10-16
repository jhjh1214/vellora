#include "canvas/LinkFocusOverlay.h"

#include "canvas/CanvasView.h"

#include <QPainter>
#include <QPen>

namespace vellora {

LinkFocusOverlay::LinkFocusOverlay(CanvasView* view, QWidget* parent)
    : QWidget(parent), m_view(view) {
    setAttribute(Qt::WA_TransparentForMouseEvents);
    setAttribute(Qt::WA_NoSystemBackground);
    setAttribute(Qt::WA_TranslucentBackground);
    connect(m_view->controller(), &CanvasController::viewChanged, this,
            qOverload<>(&QWidget::update));
    connect(m_view->links(), &LinkLayer::linksChanged, this, qOverload<>(&QWidget::update));
    connect(m_view, &CanvasView::linkFocusChanged, this, qOverload<>(&QWidget::update));
}

QRectF LinkFocusOverlay::ringRect() const {
    const auto focused = m_view->focusedLink();
    if (!focused) {
        return {};
    }
    const QList<Link> links = m_view->links()->linksOf(focused->page);
    if (focused->index < 0 || focused->index >= links.size()) {
        return {};
    }
    const QRectF rect =
        m_view->controller()->drawFor(focused->page).map(links.at(focused->index).rect);
    return rect.adjusted(-3.0, -3.0, 3.0, 3.0);
}

void LinkFocusOverlay::paintEvent(QPaintEvent*) {
    const QRectF ring = ringRect();
    if (ring.isEmpty()) {
        return;
    }
    QPainter painter(this);
    painter.setBrush(Qt::NoBrush);
    painter.setPen(QPen(Qt::white, 4.0));
    painter.drawRect(ring);
    painter.setPen(QPen(Qt::black, 2.0));
    painter.drawRect(ring);
}

} // namespace vellora
