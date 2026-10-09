#include "canvas/CanvasController.h"

#include <algorithm>
#include <cmath>

namespace vellora {

namespace {

// Sizes the engine sends at open (the first chunk of pages, see EngineSession::pageSize).
constexpr quint32 kMaxKnownSizes = 4096;
// Tiles of one request pass. A viewport needs a few dozen at most; this only bounds the work if
// the geometry (an absurdly large window, say) ever asks for more.
constexpr int kMaxTilesPerPass = 4096;
constexpr int kMaxFailedTiles = 4096;

int bucketIndex(float scale) {
    return static_cast<int>(std::lround(std::log2(scale) * 4.0));
}

} // namespace

CanvasController::CanvasController(EngineSession* session, QObject* parent)
    : QObject(parent), m_session(session) {
    connect(session, &EngineSession::opened, this, &CanvasController::onOpened);
    connect(session, &EngineSession::tileReady, this, &CanvasController::onTileReady);
    connect(session, &EngineSession::requestFailed, this,
            [this](quint64 request, const QString&) { onRequestFailed(request); });
    connect(session, &EngineSession::engineCrashed, this,
            [this](const QString&, bool, const QList<quint64>&) { onEngineCrashed(); });
    connect(session, &EngineSession::engineRestarted, this, &CanvasController::onEngineRestarted);
}

void CanvasController::reset() {
    m_layout.setPages(0, {});
    m_inFlight.clear();
    m_failed.clear();
    m_scroll = {};
    m_zoom = 1.0;
    m_reportedPage = std::numeric_limits<quint32>::max();
    emit zoomChanged(m_zoom);
    emit contentChanged();
    emit viewChanged();
}

void CanvasController::setViewportSize(QSize logicalSize) {
    if (logicalSize == m_viewport) {
        return;
    }
    m_viewport = logicalSize;
    m_scroll = clampedScroll(m_scroll);
    emit contentChanged();
    emit viewChanged();
    schedule();
    noteCurrentPage();
}

void CanvasController::setDevicePixelRatio(double ratio) {
    if (ratio <= 0.0 || ratio == m_ratio) {
        return;
    }
    m_ratio = ratio;
    emit viewChanged();
    schedule();
}

double CanvasController::contentWidth() const {
    return std::max(static_cast<double>(m_viewport.width()),
                    m_layout.maxPageWidth() * m_zoom + 2.0 * PageLayout::kGap);
}

QSizeF CanvasController::contentSize() const {
    return {contentWidth(),
            std::max(static_cast<double>(m_viewport.height()), m_layout.totalHeight(m_zoom))};
}

QPointF CanvasController::clampedScroll(QPointF position) const {
    const QSizeF content = contentSize();
    const double maxX = std::max(0.0, content.width() - m_viewport.width());
    const double maxY = std::max(0.0, content.height() - m_viewport.height());
    return {std::clamp(position.x(), 0.0, maxX), std::clamp(position.y(), 0.0, maxY)};
}

void CanvasController::setScrollPosition(QPointF position) {
    const QPointF clamped = clampedScroll(position);
    if (clamped == m_scroll) {
        return;
    }
    m_scroll = clamped;
    emit viewChanged();
    schedule();
    noteCurrentPage();
}

double CanvasController::pageLeft(quint32 page) const {
    const double width = m_layout.pageSize(page).width() * m_zoom;
    return (contentWidth() - width) / 2.0 - m_scroll.x();
}

quint32 CanvasController::currentPage() const {
    return m_layout.pageAt(m_scroll.y() + m_viewport.height() / 2.0, m_zoom);
}

void CanvasController::zoomBy(double factor, QPointF anchor) {
    setZoom(m_zoom * factor, anchor);
}

void CanvasController::setZoom(double zoom, QPointF anchor) {
    applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), anchor);
}

void CanvasController::restoreView(PageLayout::Anchor anchor, double zoom) {
    if (!std::isfinite(zoom) || !std::isfinite(anchor.offsetPoints)) {
        return;
    }
    applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), QPointF(0.0, 0.0));
    anchor.page = std::min(anchor.page, m_layout.pageCount() == 0 ? 0 : m_layout.pageCount() - 1);
    setScrollPosition({m_scroll.x(), m_layout.yOf(anchor, m_zoom)});
}

void CanvasController::actualSize() {
    applyZoom(1.0, QPointF(m_viewport.width() / 2.0, m_viewport.height() / 2.0));
}

void CanvasController::fitWidth() {
    const double width = m_layout.maxPageWidth();
    if (width <= 0.0 || m_viewport.width() <= 0) {
        return;
    }
    const double zoom = (m_viewport.width() - 2.0 * PageLayout::kGap) / width;
    applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), QPointF(0.0, 0.0));
}

void CanvasController::applyZoom(double zoom, QPointF anchor) {
    if (zoom == m_zoom) {
        return;
    }
    const PageLayout::Anchor spot = m_layout.anchorAt(m_scroll.y() + anchor.y(), m_zoom);
    const double fractionX = (m_scroll.x() + anchor.x()) / contentWidth();
    m_zoom = zoom;
    m_scroll = clampedScroll(
        {fractionX * contentWidth() - anchor.x(), m_layout.yOf(spot, m_zoom) - anchor.y()});
    emit zoomChanged(m_zoom);
    emit contentChanged();
    emit viewChanged();
    schedule();
    noteCurrentPage();
}

void CanvasController::rebuildLayout() {
    const quint32 count = m_session->pageCount();
    QVector<QSizeF> sizes;
    const quint32 known = std::min(count, kMaxKnownSizes);
    sizes.reserve(static_cast<qsizetype>(known));
    for (quint32 page = 0; page < known; ++page) {
        sizes.append(m_session->pageSize(page));
    }
    m_layout.setPages(count, sizes);
}

void CanvasController::onOpened() {
    // Also after an engine restart: same document, so the view stays where it was.
    rebuildLayout();
    m_inFlight.clear();
    m_failed.clear();
    m_scroll = clampedScroll(m_scroll);
    emit contentChanged();
    emit viewChanged();
    schedule();
    noteCurrentPage();
}

void CanvasController::onTileReady(quint64 request) {
    m_inFlight.remove(request);
    emit viewChanged();
    // A slot is free again: tiles that found the cache full can be asked for now.
    schedule();
}

void CanvasController::onRequestFailed(quint64 request) {
    if (request != 0) {
        const auto it = m_inFlight.constFind(request);
        if (it != m_inFlight.constEnd()) {
            // Not asked for again until the engine restarts or the document is reopened.
            // Bounded: a hostile engine could otherwise grow this by failing every tile.
            if (m_failed.size() >= kMaxFailedTiles) {
                m_failed.clear();
            }
            m_failed.insert(it.value());
            m_inFlight.remove(request);
        }
    }
}

void CanvasController::onEngineCrashed() {
    // The client dropped every request in flight with the engine.
    m_inFlight.clear();
}

void CanvasController::onEngineRestarted() {
    m_failed.clear();
}

void CanvasController::noteCurrentPage() {
    const quint32 count = m_layout.pageCount();
    const quint32 page = count == 0 ? 0 : currentPage();
    if (page == m_reportedPage) {
        return;
    }
    m_reportedPage = page;
    emit currentPageChanged(page, count);
}

QVector<TileDraw> CanvasController::tilesIn(double top, double bottom) const {
    QVector<TileDraw> tiles;
    if (m_layout.pageCount() == 0 || m_viewport.isEmpty()) {
        return tiles;
    }
    const double zoom = m_zoom;
    const float exact = static_cast<float>(zoom * m_ratio);
    const float scale = EngineSession::bucketScale(exact);
    if (scale <= 0.0F) {
        return tiles;
    }
    const int bucket = bucketIndex(scale);
    const double tilePts = static_cast<double>(EngineSession::tilePixels()) / scale;

    const PageLayout::Range range =
        m_layout.pagesIn(m_scroll.y() + top, m_scroll.y() + bottom, zoom);
    for (quint32 i = 0; i < range.count; ++i) {
        const quint32 page = range.first + i;
        const QSizeF size = m_layout.pageSize(page);
        const double left = pageLeft(page);
        const double pageTop = m_layout.pageTop(page, zoom) - m_scroll.y();

        // The part of the page inside the region, in points.
        const double x0 = std::max(0.0, -left / zoom);
        const double x1 = std::min(size.width(), (m_viewport.width() - left) / zoom);
        const double y0 = std::max(0.0, (top - pageTop) / zoom);
        const double y1 = std::min(size.height(), (bottom - pageTop) / zoom);
        if (x1 <= x0 || y1 <= y0) {
            continue;
        }
        const auto firstX = static_cast<quint32>(std::floor(x0 / tilePts));
        const auto lastX = static_cast<quint32>(std::ceil(x1 / tilePts)) - 1;
        const auto firstY = static_cast<quint32>(std::floor(y0 / tilePts));
        const auto lastY = static_cast<quint32>(std::ceil(y1 / tilePts)) - 1;
        for (quint32 ty = firstY; ty <= lastY; ++ty) {
            for (quint32 tx = firstX; tx <= lastX; ++tx) {
                if (tiles.size() >= kMaxTilesPerPass) {
                    return tiles;
                }
                const QRectF all(tx * tilePts, ty * tilePts, tilePts, tilePts);
                const QRectF shown = all.intersected(QRectF(0.0, 0.0, size.width(), size.height()));
                if (shown.isEmpty()) {
                    continue;
                }
                TileDraw tile;
                tile.page = page;
                tile.x = tx;
                tile.y = ty;
                tile.bucket = bucket;
                tile.scale = exact;
                tile.dest = QRectF(left + shown.x() * zoom, pageTop + shown.y() * zoom,
                                   shown.width() * zoom, shown.height() * zoom);
                tile.uv = QRectF((shown.x() - all.x()) / tilePts, (shown.y() - all.y()) / tilePts,
                                 shown.width() / tilePts, shown.height() / tilePts);
                tiles.append(tile);
            }
        }
    }
    return tiles;
}

Frame CanvasController::frame() const {
    Frame frame;
    if (m_layout.pageCount() == 0 || m_viewport.isEmpty()) {
        return frame;
    }
    const PageLayout::Range range =
        m_layout.pagesIn(m_scroll.y(), m_scroll.y() + m_viewport.height(), m_zoom);
    for (quint32 i = 0; i < range.count; ++i) {
        const quint32 page = range.first + i;
        const QSizeF size = m_layout.pageSize(page);
        frame.pages.append(
            {page, QRectF(pageLeft(page), m_layout.pageTop(page, m_zoom) - m_scroll.y(),
                          size.width() * m_zoom, size.height() * m_zoom)});
    }
    frame.tiles = tilesIn(0.0, m_viewport.height());
    return frame;
}

void CanvasController::schedule() {
    if (!m_session->isOpen() || m_layout.pageCount() == 0 || m_viewport.isEmpty()) {
        return;
    }
    const double height = m_viewport.height();
    const QVector<TileDraw> visible = tilesIn(0.0, height);
    QVector<TileDraw> around = tilesIn(-height, 2.0 * height);

    const auto idOf = [](const TileDraw& t) { return t.id(); };
    QSet<TileId> wanted;
    for (const TileDraw& tile : visible) {
        wanted.insert(idOf(tile));
    }
    // Prefetch: what is near but not on screen, nearest the viewport first.
    QVector<TileDraw> prefetch;
    for (const TileDraw& tile : around) {
        if (!wanted.contains(idOf(tile))) {
            prefetch.append(tile);
        }
    }
    const double middle = height / 2.0;
    std::stable_sort(
        prefetch.begin(), prefetch.end(), [middle](const TileDraw& a, const TileDraw& b) {
            return std::abs(a.dest.center().y() - middle) < std::abs(b.dest.center().y() - middle);
        });
    if (prefetch.size() > kMaxPrefetchTiles) {
        prefetch.resize(kMaxPrefetchTiles);
    }
    for (const TileDraw& tile : prefetch) {
        wanted.insert(idOf(tile));
    }

    // Withdraw what scrolling or zooming made pointless, before asking for more.
    for (auto it = m_inFlight.begin(); it != m_inFlight.end();) {
        if (wanted.contains(it.value())) {
            ++it;
        } else {
            m_session->cancel(it.key());
            it = m_inFlight.erase(it);
        }
    }

    const auto ask = [this, &idOf](const TileDraw& tile, TilePriority priority) {
        const TileId id = idOf(tile);
        if (m_failed.contains(id)) {
            return true;
        }
        const TileTicket ticket =
            m_session->requestTile(tile.page, tile.scale, tile.x, tile.y, priority);
        switch (ticket.state) {
        case TileState::Requested:
            m_inFlight.insert(ticket.request, id);
            return true;
        case TileState::Full:
            return false; // every slot is busy; tileReady brings us back here
        default:
            return true; // Ready, or InFlight from an earlier pass
        }
    };
    for (const TileDraw& tile : visible) {
        if (!ask(tile, TilePriority::Visible)) {
            return;
        }
    }
    for (const TileDraw& tile : prefetch) {
        if (!ask(tile, TilePriority::Prefetch)) {
            return;
        }
    }
}

} // namespace vellora
