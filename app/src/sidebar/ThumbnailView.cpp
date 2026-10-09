#include "sidebar/ThumbnailView.h"

#include <QElapsedTimer>
#include <QFocusEvent>
#include <QHideEvent>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPaintEvent>
#include <QPainter>
#include <QScrollBar>
#include <QShowEvent>
#include <QWheelEvent>
#include <algorithm>
#include <cmath>
#include <cstdlib>

namespace vellora {

namespace {

constexpr int kPadding = 6;            // around a thumbnail, above and below
constexpr int kLabelGap = 2;           // between a thumbnail and its label
constexpr int kWheelStep = 8;          // Ctrl+wheel: logical pixels of width per notch
constexpr int kSingleStep = 40;        // arrow keys on the scroll bar
constexpr float kBucketStep = 0.8409F; // one scale bucket down (2^-1/4)
constexpr float kMaxScale = 64.0F;     // `vellora_ipc::MAX_TILE_SCALE`

} // namespace

ThumbnailView::ThumbnailView(EngineSession* session, CanvasController* controller, QWidget* parent)
    : QAbstractScrollArea(parent), m_session(session), m_controller(controller) {
    setFocusPolicy(Qt::StrongFocus);
    setFrameShape(QFrame::NoFrame);
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    setVerticalScrollBarPolicy(Qt::ScrollBarAsNeeded);
    setAccessibleName(tr("Page thumbnails"));
    viewport()->setBackgroundRole(QPalette::Base);
    viewport()->setAutoFillBackground(true);
    verticalScrollBar()->setSingleStep(kSingleStep);
    m_settle.setSingleShot(true);
    m_settle.setInterval(kSettleMs);
    connect(&m_settle, &QTimer::timeout, this, [this] {
        m_flinging = false;
        scheduleSoon();
    });

    connect(m_session, &EngineSession::opened, this, &ThumbnailView::onOpened);
    connect(m_session, &EngineSession::tileReady, this, &ThumbnailView::onTileReady);
    connect(m_session, &EngineSession::requestFailed, this, &ThumbnailView::onRequestFailed);
    connect(m_session, &EngineSession::engineCrashed, this, &ThumbnailView::onEngineCrashed);
    connect(m_session, &EngineSession::engineRestarted, this, &ThumbnailView::onEngineRestarted);
    connect(m_controller, &CanvasController::currentPageChanged, this,
            [this](quint32 page, quint32) { onCurrentPageChanged(page); });
    connect(m_controller, &CanvasController::viewModeChanged, this,
            [this](CanvasController::ViewMode) { viewport()->update(); });
}

void ThumbnailView::reset() {
    m_inFlight.clear();
    m_failed.clear();
    m_labels.clear();
    m_pageCount = 0;
    m_tops.clear();
    m_highlight = 0;
    m_selected = 0;
    m_requestsSent = 0;
    verticalScrollBar()->setValue(0);
    updateScrollBars();
    viewport()->update();
}

// ---- geometry ----

QSize ThumbnailView::imageSize(quint32 page) const {
    const QSizeF file = m_controller->layout().pageSize(page);
    // The page's width fits the thumbnail's, unless that makes it taller than `kMaxAspect` widths.
    const double scale = std::min(m_width / file.width(), m_width * kMaxAspect / file.height());
    return {std::max(1, qRound(file.width() * scale)), std::max(1, qRound(file.height() * scale))};
}

ThumbnailView::Render ThumbnailView::renderFor(quint32 page) const {
    const QSizeF file = m_controller->layout().pageSize(page);
    const QSize shown = imageSize(page);
    const double side = EngineSession::tilePixels();
    // Device pixels per point: what the thumbnail needs, and at most what fits one tile.
    double want = shown.width() / file.width() * devicePixelRatioF();
    want = std::min(
        {want, side / std::max(file.width(), file.height()), static_cast<double>(kMaxScale)});
    float zoom = EngineSession::bucketScale(static_cast<float>(want));
    const auto sizeAt = [&file](float scale) {
        return QSize(static_cast<int>(std::ceil(file.width() * scale)),
                     static_cast<int>(std::ceil(file.height() * scale)));
    };
    // The bucket is the nearest one, which can be a little larger than `want`: step down until the
    // page fits a tile.
    for (int i = 0; i < 16 && zoom > 0.0F; ++i) {
        const QSize size = sizeAt(zoom);
        if (size.width() <= side && size.height() <= side) {
            return {zoom, static_cast<quint32>(std::max(1, size.width())),
                    static_cast<quint32>(std::max(1, size.height()))};
        }
        zoom = EngineSession::bucketScale(zoom * kBucketStep);
    }
    return {};
}

void ThumbnailView::rebuildGeometry() {
    m_pageCount = m_controller->pageCount();
    m_tops.resize(static_cast<qsizetype>(m_pageCount) + 1);
    const int label = fontMetrics().height();
    double top = 0.0;
    for (quint32 page = 0; page < m_pageCount; ++page) {
        m_tops[static_cast<qsizetype>(page)] = top;
        top += imageSize(page).height() + 2 * kPadding + kLabelGap + label;
    }
    m_tops[static_cast<qsizetype>(m_pageCount)] = top;
    updateScrollBars();
}

void ThumbnailView::updateScrollBars() {
    QScrollBar* bar = verticalScrollBar();
    const int viewHeight = viewport()->height();
    const int total = static_cast<int>(std::min(contentHeight(), 1.0e9));
    bar->setRange(0, std::max(0, total - viewHeight));
    bar->setPageStep(viewHeight);
}

double ThumbnailView::contentHeight() const {
    return m_tops.isEmpty() ? 0.0 : m_tops.last();
}

QRect ThumbnailView::cellRect(quint32 page) const {
    if (page >= m_pageCount) {
        return {};
    }
    const double top = m_tops.at(static_cast<qsizetype>(page));
    const double bottom = m_tops.at(static_cast<qsizetype>(page) + 1);
    return {0, qRound(top) - verticalScrollBar()->value(), viewport()->width(),
            qRound(bottom) - qRound(top)};
}

QRect ThumbnailView::imageRect(quint32 page) const {
    const QRect cell = cellRect(page);
    if (cell.isNull()) {
        return {};
    }
    const QSize size = imageSize(page);
    return {cell.x() + (cell.width() - size.width()) / 2, cell.y() + kPadding, size.width(),
            size.height()};
}

quint32 ThumbnailView::pageAt(QPoint position) const {
    if (m_pageCount == 0) {
        return 0;
    }
    const double y = position.y() + verticalScrollBar()->value();
    if (y < 0.0 || y >= m_tops.last()) {
        return m_pageCount;
    }
    const auto it = std::upper_bound(m_tops.constBegin(), m_tops.constEnd() - 1, y);
    return static_cast<quint32>(it - m_tops.constBegin() - 1);
}

ThumbnailView::Range ThumbnailView::visiblePages() const {
    if (m_pageCount == 0) {
        return {};
    }
    const double top = verticalScrollBar()->value();
    const double bottom = top + viewport()->height();
    const auto first = std::upper_bound(m_tops.constBegin(), m_tops.constEnd() - 1, top);
    const auto end = std::lower_bound(m_tops.constBegin(), m_tops.constEnd() - 1, bottom);
    const auto firstPage =
        static_cast<quint32>(std::max<qsizetype>(0, first - m_tops.constBegin() - 1));
    return {firstPage, std::max(firstPage, static_cast<quint32>(end - m_tops.constBegin()))};
}

void ThumbnailView::ensureVisible(quint32 page) {
    if (page >= m_pageCount) {
        return;
    }
    const double top = m_tops.at(static_cast<qsizetype>(page));
    const double bottom = m_tops.at(static_cast<qsizetype>(page) + 1);
    QScrollBar* bar = verticalScrollBar();
    if (top < bar->value()) {
        bar->setValue(qFloor(top));
    } else if (bottom > bar->value() + viewport()->height()) {
        bar->setValue(qCeil(bottom) - viewport()->height());
    }
}

// ---- size ----

void ThumbnailView::setThumbnailWidth(int width) {
    width = std::clamp(width, kMinWidth, kMaxWidth);
    if (width == m_width) {
        return;
    }
    // Keep the page at the top of the view where it is, however the cells change size.
    const Range seen = visiblePages();
    double fraction = 0.0;
    if (seen.first < m_pageCount) {
        const double top = m_tops.at(static_cast<qsizetype>(seen.first));
        const double height = m_tops.at(static_cast<qsizetype>(seen.first) + 1) - top;
        fraction = height > 0.0 ? (verticalScrollBar()->value() - top) / height : 0.0;
    }
    m_width = width;
    rebuildGeometry();
    if (seen.first < m_pageCount) {
        const double top = m_tops.at(static_cast<qsizetype>(seen.first));
        const double height = m_tops.at(static_cast<qsizetype>(seen.first) + 1) - top;
        verticalScrollBar()->setValue(qRound(top + fraction * height));
    }
    viewport()->update();
    scheduleSoon();
    emit thumbnailWidthChanged(m_width);
}

QString ThumbnailView::labelFor(quint32 page) const {
    if (page < static_cast<quint32>(m_labels.size()) && !m_labels.at(page).isEmpty()) {
        return m_labels.at(page);
    }
    return QString::number(static_cast<qulonglong>(page) + 1);
}

void ThumbnailView::setPageLabels(const QStringList& labels) {
    m_labels = labels;
    viewport()->update();
}

// ---- following and driving the canvas ----

bool ThumbnailView::isHighlighted(quint32 page) const {
    const PageLayout& layout = m_controller->layout();
    if (page >= layout.pageCount() || m_highlight >= layout.pageCount()) {
        return page == m_highlight;
    }
    return layout.rowOf(page) == layout.rowOf(m_highlight);
}

void ThumbnailView::onCurrentPageChanged(quint32 page) {
    if (m_jumping) {
        return; // our own jump: the highlight stays on the page the user chose
    }
    m_highlight = page;
    m_selected = page;
    ensureVisible(page);
    viewport()->update();
}

void ThumbnailView::select(quint32 page) {
    if (m_pageCount == 0) {
        return;
    }
    page = std::min(page, m_pageCount - 1);
    m_selected = page;
    m_highlight = page;
    ensureVisible(page);
    viewport()->update();
}

void ThumbnailView::goToPage(quint32 page) {
    if (m_pageCount == 0) {
        return;
    }
    select(page);
    m_jumping = true;
    m_controller->goToPage(m_selected);
    m_jumping = false;
}

// ---- engine events ----

void ThumbnailView::onOpened() {
    // Also the answer of a restarted engine: the same document, so the view stays where it is.
    const quint32 before = m_pageCount;
    rebuildGeometry();
    if (before != m_pageCount) {
        m_highlight = std::min(m_highlight, m_pageCount == 0 ? 0 : m_pageCount - 1);
        m_selected = std::min(m_selected, m_pageCount == 0 ? 0 : m_pageCount - 1);
    }
    viewport()->update();
    scheduleSoon();
}

void ThumbnailView::onTileReady(quint64 request) {
    if (m_inFlight.remove(request)) {
        viewport()->update();
        scheduleSoon();
    }
}

void ThumbnailView::onRequestFailed(quint64 request) {
    const auto it = m_inFlight.constFind(request);
    if (it == m_inFlight.constEnd()) {
        return;
    }
    // Not asked for again until the engine restarts. Bounded, as in the canvas.
    if (m_failed.size() >= 4096) {
        m_failed.clear();
    }
    m_failed.insert(it.value());
    m_inFlight.erase(it);
}

void ThumbnailView::onEngineCrashed() {
    m_inFlight.clear();
}

void ThumbnailView::onEngineRestarted() {
    m_failed.clear();
    scheduleSoon();
}

void ThumbnailView::scheduleSoon() {
    if (!m_scheduleQueued) {
        m_scheduleQueued = true;
        QMetaObject::invokeMethod(this, &ThumbnailView::schedule, Qt::QueuedConnection);
    }
}

void ThumbnailView::schedule() {
    m_scheduleQueued = false;
    // A hidden sidebar (or the sidebar of a tab in the background) asks for nothing.
    if (!isVisible() || !m_session->isOpen() || m_pageCount == 0 || viewport()->height() <= 0) {
        return;
    }
    // Wanted: the cells on screen, then a few below and above them, nearest first.
    QVector<quint32> wanted;
    const Range seen = visiblePages();
    for (quint32 page = seen.first; page < seen.end && wanted.size() < kMaxWanted; ++page) {
        wanted.append(page);
    }
    for (quint32 i = 0; i < static_cast<quint32>(kMargin); ++i) {
        if (seen.end + i < m_pageCount && wanted.size() < kMaxWanted) {
            wanted.append(seen.end + i);
        }
        if (seen.first > i && wanted.size() < kMaxWanted) {
            wanted.append(seen.first - 1 - i);
        }
    }

    QVector<Render> renders;
    renders.reserve(wanted.size());
    QSet<Key> keys;
    for (const quint32 page : std::as_const(wanted)) {
        renders.append(renderFor(page));
        if (renders.last().valid()) {
            keys.insert({page, renders.last().zoom});
        }
    }

    // Withdraw what scrolling or resizing made pointless, before asking for more.
    for (auto it = m_inFlight.begin(); it != m_inFlight.end();) {
        if (keys.contains(it.value())) {
            ++it;
        } else {
            m_session->cancel(it.key());
            it = m_inFlight.erase(it);
        }
    }

    if (m_flinging) {
        return; // asked for once the list has settled
    }
    int sent = 0;
    for (qsizetype i = 0; i < wanted.size(); ++i) {
        const quint32 page = wanted.at(i);
        const Render render = renders.at(i);
        const Key key{page, render.zoom};
        if (!render.valid() || m_failed.contains(key)) {
            continue;
        }
        const TileTicket ticket =
            m_session->requestThumbnail(page, render.zoom, render.width, render.height);
        switch (ticket.state) {
        case TileState::Requested:
            m_inFlight.insert(ticket.request, key);
            ++m_requestsSent;
            if (++sent >= kMaxNewPerPass) {
                scheduleSoon(); // the rest follows; the work of one pass stays small
                return;
            }
            break;
        case TileState::Full:
            return; // every slot is busy; `tileReady` brings us back
        default:
            break; // Ready, or in flight from an earlier pass
        }
    }
}

bool ThumbnailView::hasThumbnail(quint32 page) {
    if (page >= m_pageCount) {
        return false;
    }
    const Render render = renderFor(page);
    QImage image;
    return render.valid() &&
           m_session->readThumbnail(page, render.zoom, render.width, render.height, image);
}

// ---- events ----

void ThumbnailView::paintEvent(QPaintEvent*) {
    QElapsedTimer timer;
    timer.start();
    QPainter painter(viewport());
    painter.setRenderHint(QPainter::SmoothPixmapTransform);
    const QRect visible = viewport()->rect();
    const Range seen = visiblePages();
    const int labelHeight = fontMetrics().height();
    QImage image;
    for (quint32 page = seen.first; page < seen.end; ++page) {
        const QRect cell = cellRect(page);
        if (!cell.intersects(visible)) {
            continue;
        }
        const bool current = isHighlighted(page);
        if (current) {
            QColor fill = palette().color(QPalette::Highlight);
            fill.setAlpha(90);
            painter.fillRect(cell.adjusted(2, 1, -2, -1), fill);
        }
        const QRect target = imageRect(page);
        const Render render = renderFor(page);
        if (render.valid() &&
            m_session->readThumbnail(page, render.zoom, render.width, render.height, image)) {
            painter.drawImage(target, image);
        } else {
            painter.fillRect(target, palette().color(QPalette::AlternateBase));
        }
        painter.setPen(palette().color(QPalette::Mid));
        painter.drawRect(target.adjusted(0, 0, -1, -1));

        const QRect labelRect(cell.x(), target.bottom() + 1 + kLabelGap, cell.width(), labelHeight);
        QFont font = painter.font();
        font.setBold(current);
        painter.setFont(font);
        painter.setPen(palette().color(QPalette::Text));
        painter.drawText(labelRect, Qt::AlignHCenter | Qt::AlignTop,
                         painter.fontMetrics().elidedText(labelFor(page), Qt::ElideRight,
                                                          cell.width() - 2 * kPadding));
        if (page == m_selected && hasFocus()) {
            painter.setPen(QPen(palette().color(QPalette::Highlight), 1, Qt::DotLine));
            painter.drawRect(cell.adjusted(2, 1, -3, -2));
        }
        ++m_paintCount;
    }
    emit paintTimed(static_cast<double>(timer.nsecsElapsed()) / 1.0e6);
}

void ThumbnailView::resizeEvent(QResizeEvent* event) {
    QAbstractScrollArea::resizeEvent(event);
    updateScrollBars();
    scheduleSoon();
}

void ThumbnailView::scrollContentsBy(int, int dy) {
    if (std::abs(dy) >= viewport()->height() / 2) {
        m_flinging = true;
    }
    if (m_flinging) {
        m_settle.start(); // still moving: wait for it to stop
    }
    viewport()->update();
    scheduleSoon();
}

void ThumbnailView::showEvent(QShowEvent* event) {
    QAbstractScrollArea::showEvent(event);
    scheduleSoon();
}

void ThumbnailView::hideEvent(QHideEvent* event) {
    QAbstractScrollArea::hideEvent(event);
    // Nobody sees the answers: withdraw the requests the engine has not started.
    for (auto it = m_inFlight.constBegin(); it != m_inFlight.constEnd(); ++it) {
        m_session->cancel(it.key());
    }
    m_inFlight.clear();
}

void ThumbnailView::mousePressEvent(QMouseEvent* event) {
    if (event->button() == Qt::LeftButton) {
        setFocus(Qt::MouseFocusReason);
        const quint32 page = pageAt(event->position().toPoint());
        if (page < m_pageCount) {
            goToPage(page);
        }
        event->accept();
        return;
    }
    QAbstractScrollArea::mousePressEvent(event);
}

void ThumbnailView::keyPressEvent(QKeyEvent* event) {
    if (m_pageCount == 0 || (event->modifiers() & (Qt::ControlModifier | Qt::AltModifier))) {
        QAbstractScrollArea::keyPressEvent(event);
        return;
    }
    const Range seen = visiblePages();
    const quint32 shown = seen.end - seen.first;
    const quint32 pageStep = shown > 1 ? shown - 1 : 1;
    const quint32 last = m_pageCount - 1;
    switch (event->key()) {
    case Qt::Key_Up:
        goToPage(m_selected > 0 ? m_selected - 1 : 0);
        break;
    case Qt::Key_Down:
        goToPage(std::min(m_selected + 1, last));
        break;
    case Qt::Key_PageUp:
        goToPage(m_selected > pageStep ? m_selected - pageStep : 0);
        break;
    case Qt::Key_PageDown:
        goToPage(std::min(m_selected + pageStep, last));
        break;
    case Qt::Key_Home:
        goToPage(0);
        break;
    case Qt::Key_End:
        goToPage(last);
        break;
    case Qt::Key_Return:
    case Qt::Key_Enter:
        goToPage(m_selected);
        break;
    default:
        QAbstractScrollArea::keyPressEvent(event);
        return;
    }
    event->accept();
}

void ThumbnailView::wheelEvent(QWheelEvent* event) {
    if (event->modifiers() & Qt::ControlModifier) {
        const int notches = event->angleDelta().y() / 120;
        if (notches != 0) {
            setThumbnailWidth(m_width + notches * kWheelStep);
        }
        event->accept();
        return;
    }
    QAbstractScrollArea::wheelEvent(event);
}

void ThumbnailView::focusInEvent(QFocusEvent* event) {
    QAbstractScrollArea::focusInEvent(event);
    viewport()->update();
}

void ThumbnailView::focusOutEvent(QFocusEvent* event) {
    QAbstractScrollArea::focusOutEvent(event);
    viewport()->update();
}

} // namespace vellora
