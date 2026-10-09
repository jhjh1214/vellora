#include "canvas/CanvasView.h"

#include "canvas/CanvasWidget.h"

#include <QEvent>
#include <QKeyEvent>
#include <QLabel>
#include <QMouseEvent>
#include <QRubberBand>
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
    // Always shown (disabled when there is nothing to scroll): a scroll bar that comes and goes as
    // the zoom or the page changes resizes the viewport, and with it the GPU surface, which costs
    // a frame (measured: 8-9 ms in a turned view, whose pages overflow at 125% but not at 100%).
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOn);
    setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOn);
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
    connect(&m_controller, &CanvasController::viewModeChanged, this, [this] { syncScrollBars(); });
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
    // The vertical range is the whole column when scrolling continuously, else the row shown.
    const CanvasController::VerticalRange range = m_controller.verticalRange();
    verticalScrollBar()->setRange(clampToInt(range.min), clampToInt(range.max));
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
    if (!m_controller.viewMode().continuous) {
        // Turning pages: scrolling past the end of a row shows the next one (the controller
        // decides), which the scroll bar alone cannot do.
        QPointF delta;
        if (!event->pixelDelta().isNull()) {
            delta = -QPointF(event->pixelDelta());
        } else {
            constexpr double kLinesPerNotch = 3.0;
            delta = -QPointF(event->angleDelta()) / 120.0 * kLinesPerNotch * kSingleStep;
        }
        m_controller.scrollBy(delta);
        event->accept();
        return;
    }
    QAbstractScrollArea::wheelEvent(event);
}

void CanvasView::keyPressEvent(QKeyEvent* event) {
    if (event->key() == Qt::Key_Z && event->modifiers() == Qt::NoModifier) {
        if (!event->isAutoRepeat()) {
            m_zKeyDown = true;
            updateCursor();
        }
        event->accept();
        return;
    }
    if (event->key() == Qt::Key_Escape && (m_dragging || zoomRectArmed())) {
        endDrag(false);
        armZoomRect(false);
        event->accept();
        return;
    }
    if (!m_controller.viewMode().continuous && event->modifiers() == Qt::NoModifier &&
        (event->key() == Qt::Key_PageDown || event->key() == Qt::Key_PageUp)) {
        // A page of scrolling, and at the end of the row, the next row.
        const double page = std::max(1.0, viewport()->height() - 2.0 * kSingleStep);
        m_controller.scrollBy(QPointF(0.0, event->key() == Qt::Key_PageDown ? page : -page));
        event->accept();
        return;
    }
    QAbstractScrollArea::keyPressEvent(event);
}

void CanvasView::keyReleaseEvent(QKeyEvent* event) {
    if (event->key() == Qt::Key_Z && !event->isAutoRepeat()) {
        m_zKeyDown = false;
        updateCursor();
        event->accept();
        return;
    }
    QAbstractScrollArea::keyReleaseEvent(event);
}

void CanvasView::focusOutEvent(QFocusEvent* event) {
    // The release of Z would go to whoever has the focus now.
    m_zKeyDown = false;
    endDrag(false);
    updateCursor();
    QAbstractScrollArea::focusOutEvent(event);
}

void CanvasView::armZoomRect(bool armed) {
    m_zoomRectOneShot = armed;
    updateCursor();
}

QRect CanvasView::dragRect() const {
    return m_dragging && m_band != nullptr ? m_band->geometry() : QRect();
}

void CanvasView::updateCursor() {
    viewport()->setCursor(zoomRectArmed() ? Qt::CrossCursor : Qt::ArrowCursor);
}

void CanvasView::mousePressEvent(QMouseEvent* event) {
    if (event->button() == Qt::LeftButton && zoomRectArmed()) {
        m_dragging = true;
        m_dragStart = event->position().toPoint();
        if (m_band == nullptr) {
            m_band = new QRubberBand(QRubberBand::Rectangle, viewport());
        }
        m_band->setGeometry(QRect(m_dragStart, QSize()));
        m_band->show();
        event->accept();
        return;
    }
    QAbstractScrollArea::mousePressEvent(event);
}

void CanvasView::mouseMoveEvent(QMouseEvent* event) {
    if (m_dragging) {
        m_band->setGeometry(QRect(m_dragStart, event->position().toPoint()).normalized());
        event->accept();
        return;
    }
    QAbstractScrollArea::mouseMoveEvent(event);
}

void CanvasView::mouseReleaseEvent(QMouseEvent* event) {
    if (m_dragging && event->button() == Qt::LeftButton) {
        m_band->setGeometry(QRect(m_dragStart, event->position().toPoint()).normalized());
        endDrag(true);
        event->accept();
        return;
    }
    QAbstractScrollArea::mouseReleaseEvent(event);
}

void CanvasView::endDrag(bool zoom) {
    if (!m_dragging) {
        return;
    }
    m_dragging = false;
    const QRect rect = m_band->geometry();
    m_band->hide();
    // A drag of a few pixels is a click, not a rectangle.
    constexpr int kSmallest = 6;
    if (zoom && rect.width() >= kSmallest && rect.height() >= kSmallest) {
        m_controller.zoomToRect(QRectF(rect));
    }
    // The tool serves one drag when it was armed by the command; the key keeps it while down.
    m_zoomRectOneShot = false;
    updateCursor();
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

void CanvasView::fitPage() {
    m_controller.fitPage();
}

} // namespace vellora
