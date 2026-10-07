#include "canvas/CanvasView.h"

#include "canvas/CanvasWidget.h"

#include <QEvent>
#include <QLabel>
#include <QScrollBar>
#include <QWheelEvent>
#include <algorithm>
#include <cmath>
#include <limits>

namespace vellora {

namespace {

constexpr int kSingleStep = 40; // logical pixels per arrow key press or wheel line

// Scroll bars count in `int`; a column of ten thousand pages at 800% zoom is far below the limit.
int clampToInt(double value) {
    constexpr double kLimit = std::numeric_limits<int>::max() / 2.0;
    return static_cast<int>(std::clamp(value, 0.0, kLimit));
}

} // namespace

CanvasView::CanvasView(EngineSession* session, QWidget* parent)
    : QAbstractScrollArea(parent), m_controller(session), m_canvas(nullptr) {
    // The canvas is a child of the viewport, not the viewport itself: the scroll area consumes the
    // viewport's paint events, so a QRhiWidget used as viewport would never be asked to render.
    m_canvas = new CanvasWidget(session, &m_controller, viewport());
    m_banner = new QLabel(viewport());
    m_banner->setTextFormat(Qt::PlainText); // engine-derived text must never be read as markup
    m_banner->setAttribute(Qt::WA_TransparentForMouseEvents);
    m_banner->setStyleSheet(QStringLiteral("background: rgba(32, 32, 32, 220); color: white;"
                                           "padding: 6px 14px; border-radius: 4px;"));
    m_banner->setWordWrap(true);
    m_banner->hide();
    setFocusPolicy(Qt::StrongFocus);
    verticalScrollBar()->setSingleStep(kSingleStep);
    horizontalScrollBar()->setSingleStep(kSingleStep);
    m_controller.setDevicePixelRatio(m_canvas->devicePixelRatioF());

    const auto scrolled = [this] {
        if (!m_syncing) {
            m_controller.setScrollPosition({static_cast<double>(horizontalScrollBar()->value()),
                                            static_cast<double>(verticalScrollBar()->value())});
        }
    };
    connect(verticalScrollBar(), &QScrollBar::valueChanged, this, scrolled);
    connect(horizontalScrollBar(), &QScrollBar::valueChanged, this, scrolled);
    connect(&m_controller, &CanvasController::contentChanged, this, &CanvasView::syncScrollBars);
    connect(&m_controller, &CanvasController::viewChanged, this, &CanvasView::syncScrollBars);
}

void CanvasView::reset() {
    m_controller.reset();
}

void CanvasView::syncScrollBars() {
    // Setting ranges and values from the controller must not read back as the user scrolling.
    m_syncing = true;
    const QSizeF content = m_controller.contentSize();
    const QSize view = viewport()->size();
    verticalScrollBar()->setRange(0, clampToInt(content.height() - view.height()));
    verticalScrollBar()->setPageStep(view.height());
    horizontalScrollBar()->setRange(0, clampToInt(content.width() - view.width()));
    horizontalScrollBar()->setPageStep(view.width());
    const QPointF scroll = m_controller.scrollPosition();
    verticalScrollBar()->setValue(clampToInt(scroll.y()));
    horizontalScrollBar()->setValue(clampToInt(scroll.x()));
    m_syncing = false;
}

bool CanvasView::viewportEvent(QEvent* event) {
    if (event->type() == QEvent::Resize) {
        // Also when a scroll bar appears or disappears, which resizes only the viewport.
        m_canvas->setGeometry(viewport()->rect());
        placeBanner();
        m_controller.setViewportSize(viewport()->size());
        syncScrollBars();
    }
    return QAbstractScrollArea::viewportEvent(event);
}

void CanvasView::wheelEvent(QWheelEvent* event) {
    if (event->modifiers().testFlag(Qt::ControlModifier)) {
        const double notches = event->angleDelta().y() / 120.0;
        if (notches != 0.0) {
            m_controller.zoomBy(std::pow(kWheelZoomPerNotch, notches), event->position());
        }
        event->accept();
        return;
    }
    QAbstractScrollArea::wheelEvent(event);
}

void CanvasView::showBanner(const QString& text) {
    m_banner->setText(text);
    m_banner->adjustSize();
    placeBanner();
    m_banner->show();
    m_banner->raise(); // above the canvas
}

void CanvasView::hideBanner() {
    m_banner->hide();
}

QString CanvasView::bannerText() const {
    return m_banner->text();
}

bool CanvasView::bannerVisible() const {
    return !m_banner->isHidden();
}

void CanvasView::placeBanner() {
    constexpr int kTopMargin = 12;
    const int maxWidth = std::max(0, viewport()->width() - 24);
    m_banner->setMaximumWidth(maxWidth);
    m_banner->adjustSize();
    m_banner->move((viewport()->width() - m_banner->width()) / 2, kTopMargin);
}

void CanvasView::zoomIn() {
    m_controller.zoomBy(kZoomStep, QPointF(viewport()->width() / 2.0, viewport()->height() / 2.0));
}

void CanvasView::zoomOut() {
    m_controller.zoomBy(1.0 / kZoomStep,
                        QPointF(viewport()->width() / 2.0, viewport()->height() / 2.0));
}

void CanvasView::actualSize() {
    m_controller.actualSize();
}

void CanvasView::fitWidth() {
    m_controller.fitWidth();
}

} // namespace vellora
