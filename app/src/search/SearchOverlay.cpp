#include "search/SearchOverlay.h"

#include <QPainter>

namespace vellora {

namespace {

const QColor kHitColor(255, 213, 0, 110);
const QColor kCurrentColor(255, 140, 0, 170);

} // namespace

SearchOverlay::SearchOverlay(CanvasController* controller, QWidget* parent)
    : QWidget(parent), m_controller(controller) {
    setAttribute(Qt::WA_TransparentForMouseEvents);
    setAttribute(Qt::WA_NoSystemBackground);
    setAttribute(Qt::WA_TranslucentBackground);
    connect(m_controller, &CanvasController::viewChanged, this, qOverload<>(&QWidget::update));
}

void SearchOverlay::setSearch(SearchController* search) {
    if (m_search != nullptr) {
        m_search->disconnect(this);
    }
    m_search = search;
    if (m_search != nullptr) {
        connect(m_search, &SearchController::hitsAdded, this, qOverload<>(&QWidget::update));
        connect(m_search, &SearchController::cleared, this, qOverload<>(&QWidget::update));
        connect(m_search, &SearchController::currentChanged, this, qOverload<>(&QWidget::update));
    }
    update();
}

QList<SearchOverlay::Highlight> SearchOverlay::highlights() const {
    QList<Highlight> out;
    if (m_search == nullptr || m_search->count() == 0) {
        return out;
    }
    for (const PageDraw& page : m_controller->visiblePages()) {
        const auto [first, last] = m_search->hitsOnPage(page.page);
        for (int i = first; i < last; ++i) {
            for (const QRectF& box : m_search->hits().at(i).boxes) {
                out.append({page.map(box), i == m_search->current()});
            }
        }
    }
    return out;
}

void SearchOverlay::paintEvent(QPaintEvent*) {
    const QList<Highlight> all = highlights();
    if (all.isEmpty()) {
        return;
    }
    QPainter painter(this);
    // The current hit last, so that nothing covers it.
    for (const bool current : {false, true}) {
        for (const Highlight& highlight : all) {
            if (highlight.current == current) {
                painter.fillRect(highlight.rect, current ? kCurrentColor : kHitColor);
            }
        }
    }
}

} // namespace vellora
