#include "text/SelectionOverlay.h"

#include "text/CaretBrowser.h"

#include <QPainter>
#include <QPalette>
#include <algorithm>

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

void SelectionOverlay::setCaretBrowser(CaretBrowser* caret) {
    m_caret = caret;
    if (m_caret != nullptr) {
        connect(m_caret, &CaretBrowser::changed, this, qOverload<>(&QWidget::update));
    }
}

QRectF SelectionOverlay::caretRect() const {
    if (m_caret == nullptr) {
        return {};
    }
    const auto caret = m_caret->caret();
    if (!caret || caret->rect.isEmpty()) {
        return {};
    }
    for (const PageDraw& page : m_controller->visiblePages()) {
        if (page.page == caret->page) {
            return page.map(caret->rect);
        }
    }
    return {};
}

void SelectionOverlay::paintEvent(QPaintEvent*) {
    const QList<QRectF> rects = highlights();
    const QRectF caret = caretRect();
    if (rects.isEmpty() && caret.isEmpty()) {
        return;
    }
    QPainter painter(this);
    // The caret is a solid line in the text colour, wide enough to find: it does not blink, which
    // keeps it still for readers who need that.
    if (!caret.isEmpty()) {
        painter.fillRect(caret.adjusted(0.0, 0.0, std::max(0.0, 2.0 - caret.width()), 0.0),
                         palette().color(QPalette::Text));
    }
    QColor color = palette().color(QPalette::Highlight);
    color.setAlpha(110);
    for (const QRectF& rect : rects) {
        painter.fillRect(rect, color);
    }
}

} // namespace vellora
