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

int quarterTurns(int turns) {
    return ((turns % 4) + 4) % 4;
}

} // namespace

const QVector<double>& CanvasController::zoomPresets() {
    static const QVector<double> presets = {0.25, 0.5, 0.75, 1.0,  1.25, 1.5, 2.0,
                                            3.0,  4.0, 8.0,  16.0, 32.0, 64.0};
    return presets;
}

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
    m_row = 0;
    m_zoom = 1.0;
    m_reportedPage = std::numeric_limits<quint32>::max();
    setZoomMode(ZoomMode::Custom);
    emit zoomChanged(m_zoom);
    emit contentChanged();
    emit viewChanged();
}

void CanvasController::setZoomMode(ZoomMode mode) {
    if (mode != m_zoomMode) {
        m_zoomMode = mode;
        emit zoomModeChanged(mode);
    }
}

void CanvasController::setViewportSize(QSize logicalSize) {
    if (logicalSize == m_viewport) {
        return;
    }
    m_viewport = logicalSize;
    applyFit();
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

// ---- geometry ----

double CanvasController::contentWidth() const {
    const double needed = 2.0 * (m_layout.halfWidthPoints() * m_zoom + m_layout.halfFixedWidth()) +
                          2.0 * PageLayout::kGap;
    return std::max(static_cast<double>(m_viewport.width()), needed);
}

QSizeF CanvasController::contentSize() const {
    return {contentWidth(),
            std::max(static_cast<double>(m_viewport.height()), m_layout.totalHeight(m_zoom))};
}

CanvasController::VerticalRange CanvasController::verticalRange() const {
    const double view = m_viewport.height();
    if (m_layout.rowCount() == 0) {
        return {};
    }
    if (m_mode.continuous) {
        return {0.0, std::max(0.0, contentSize().height() - view)};
    }
    // One row at a time: from just above the row to just below it.
    const quint32 row = std::min(m_row, m_layout.rowCount() - 1);
    const double top = m_layout.rowTop(row, m_zoom) - PageLayout::kGap;
    const double bottom =
        m_layout.rowTop(row, m_zoom) + m_layout.rowHeight(row, m_zoom) + PageLayout::kGap;
    return {top, std::max(top, bottom - view)};
}

QPointF CanvasController::clampedScroll(QPointF position) const {
    const double maxX = std::max(0.0, contentWidth() - m_viewport.width());
    const VerticalRange range = verticalRange();
    return {std::clamp(position.x(), 0.0, maxX), std::clamp(position.y(), range.min, range.max)};
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

void CanvasController::scrollBy(QPointF delta) {
    if (m_mode.continuous || m_layout.rowCount() == 0) {
        setScrollPosition(m_scroll + delta);
        return;
    }
    const VerticalRange range = verticalRange();
    const double target = m_scroll.y() + delta.y();
    constexpr double kEdge = 0.5;
    if (delta.y() > 0.0 && target > range.max && m_row + 1 < m_layout.rowCount()) {
        if (m_scroll.y() >= range.max - kEdge) {
            showRow(m_row + 1, false);
            return;
        }
    } else if (delta.y() < 0.0 && target < range.min && m_row > 0) {
        if (m_scroll.y() <= range.min + kEdge) {
            showRow(m_row - 1, true);
            return;
        }
    }
    setScrollPosition(QPointF(m_scroll.x() + delta.x(), target));
}

QRectF CanvasController::pageScreenRect(quint32 page) const {
    return m_layout.pageRect(page, m_zoom)
        .translated(contentWidth() / 2.0 - m_scroll.x(), -m_scroll.y());
}

PageLayout::Range CanvasController::visibleRows(double top, double bottom) const {
    if (m_layout.rowCount() == 0) {
        return {};
    }
    if (!m_mode.continuous) {
        return {std::min(m_row, m_layout.rowCount() - 1), 1};
    }
    return m_layout.rowsIn(m_scroll.y() + top, m_scroll.y() + bottom, m_zoom);
}

quint32 CanvasController::currentPage() const {
    if (m_layout.rowCount() == 0) {
        return 0;
    }
    if (!m_mode.continuous) {
        return m_layout.firstPageOf(std::min(m_row, m_layout.rowCount() - 1));
    }
    return m_layout.pageAt(m_scroll.y() + m_viewport.height() / 2.0, m_zoom);
}

// ---- view mode ----

void CanvasController::setViewMode(ViewMode mode) {
    mode.rotation = quarterTurns(mode.rotation);
    if (mode == m_mode) {
        return;
    }
    const quint32 page = currentPage();
    m_mode = mode;
    m_layout.setArrangement(mode.spread, mode.rotation);
    applyArrangement();
    // A zoom that follows the window follows the new shape of the pages too.
    applyFit();
    goToPage(page);
    emit viewModeChanged(m_mode);
}

void CanvasController::setContinuous(bool continuous) {
    ViewMode mode = m_mode;
    mode.continuous = continuous;
    setViewMode(mode);
}

void CanvasController::setSpread(PageLayout::Spread spread) {
    ViewMode mode = m_mode;
    mode.spread = spread;
    setViewMode(mode);
}

void CanvasController::rotateBy(int turns) {
    setRotation(m_mode.rotation + turns);
}

void CanvasController::setRotation(int turns) {
    ViewMode mode = m_mode;
    mode.rotation = turns;
    setViewMode(mode);
}

void CanvasController::applyArrangement() {
    m_row = m_layout.rowCount() == 0 ? 0 : std::min(m_row, m_layout.rowCount() - 1);
    emit contentChanged();
    emit viewChanged();
}

// ---- zoom ----

void CanvasController::zoomBy(double factor, QPointF anchor) {
    setZoom(m_zoom * factor, anchor);
}

void CanvasController::setZoom(double zoom, QPointF anchor) {
    setZoomMode(ZoomMode::Custom);
    applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), anchor);
}

void CanvasController::actualSize() {
    setZoomMode(ZoomMode::Custom);
    applyZoom(1.0, QPointF(m_viewport.width() / 2.0, m_viewport.height() / 2.0));
}

void CanvasController::fitWidth() {
    setZoomMode(ZoomMode::FitWidth);
    applyFit();
}

void CanvasController::fitPage() {
    setZoomMode(ZoomMode::FitPage);
    applyFit();
}

void CanvasController::applyFit() {
    if (m_zoomMode == ZoomMode::Custom || m_layout.rowCount() == 0 || m_viewport.isEmpty()) {
        return;
    }
    const double gap = PageLayout::kGap;
    double zoom = 0.0;
    if (m_zoomMode == ZoomMode::FitWidth) {
        const double width = 2.0 * m_layout.halfWidthPoints();
        if (width <= 0.0) {
            return;
        }
        zoom = (m_viewport.width() - 2.0 * gap - 2.0 * m_layout.halfFixedWidth()) / width;
    } else {
        const quint32 row = m_mode.continuous
                                ? m_layout.rowAt(m_scroll.y() + m_viewport.height() / 2.0, m_zoom)
                                : std::min(m_row, m_layout.rowCount() - 1);
        const double width = m_layout.rowWidthPoints(row);
        const double height = m_layout.rowHeightPoints(row);
        if (width <= 0.0 || height <= 0.0) {
            return;
        }
        zoom = std::min((m_viewport.width() - 2.0 * gap - m_layout.rowFixedWidth(row)) / width,
                        (m_viewport.height() - 2.0 * gap) / height);
        // The row fills the window: show it from the top.
        applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), QPointF(0.0, 0.0));
        setScrollPosition({m_scroll.x(), m_layout.rowTop(row, m_zoom) - gap});
        return;
    }
    if (std::isfinite(zoom) && zoom > 0.0) {
        applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), QPointF(0.0, 0.0));
    }
}

void CanvasController::applyZoom(double zoom, QPointF anchor) {
    if (zoom == m_zoom || !std::isfinite(zoom)) {
        return;
    }
    // The place in the document under the anchor, kept there through the change of scale.
    PageLayout::Anchor spot;
    if (m_mode.continuous || m_layout.rowCount() == 0) {
        spot = m_layout.anchorAt(m_scroll.y() + anchor.y(), m_zoom);
    } else {
        const quint32 row = std::min(m_row, m_layout.rowCount() - 1);
        spot = {m_layout.firstPageOf(row),
                (m_scroll.y() + anchor.y() - m_layout.rowTop(row, m_zoom)) / m_zoom};
    }
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

void CanvasController::zoomToRect(const QRectF& rect) {
    if (m_viewport.isEmpty() || rect.width() < 1.0 || rect.height() < 1.0) {
        return;
    }
    const double factor =
        std::min(m_viewport.width() / rect.width(), m_viewport.height() / rect.height());
    setZoomMode(ZoomMode::Custom);
    const QPointF centre = rect.center();
    applyZoom(std::clamp(m_zoom * factor, kMinZoom, kMaxZoom), centre);
    // The spot under the middle of the rectangle stayed where it was; bring it to the middle.
    setScrollPosition(m_scroll + centre -
                      QPointF(m_viewport.width() / 2.0, m_viewport.height() / 2.0));
}

void CanvasController::restoreView(PageLayout::Anchor anchor, double zoom) {
    if (!std::isfinite(zoom) || !std::isfinite(anchor.offsetPoints)) {
        return;
    }
    setZoomMode(ZoomMode::Custom);
    applyZoom(std::clamp(zoom, kMinZoom, kMaxZoom), QPointF(0.0, 0.0));
    anchor.page = std::min(anchor.page, m_layout.pageCount() == 0 ? 0 : m_layout.pageCount() - 1);
    if (!m_mode.continuous && m_layout.rowCount() > 0) {
        m_row = m_layout.rowOf(anchor.page);
        emit contentChanged();
    }
    setScrollPosition({m_scroll.x(), m_layout.yOf(anchor, m_zoom)});
}

// ---- navigation ----

void CanvasController::showRow(quint32 row, bool fromBottom) {
    if (m_layout.rowCount() == 0) {
        return;
    }
    m_row = std::min(row, m_layout.rowCount() - 1);
    const VerticalRange range = verticalRange();
    m_scroll.setY(fromBottom ? range.max : range.min);
    m_scroll = clampedScroll(m_scroll);
    if (m_zoomMode == ZoomMode::FitPage) {
        applyFit();
    }
    emit contentChanged();
    emit viewChanged();
    schedule();
    noteCurrentPage();
}

void CanvasController::goToPage(quint32 page) {
    if (m_layout.rowCount() == 0) {
        return;
    }
    page = std::min(page, m_layout.pageCount() - 1);
    const quint32 row = m_layout.rowOf(page);
    if (m_mode.continuous) {
        setScrollPosition({m_scroll.x(), m_layout.rowTop(row, m_zoom) - PageLayout::kGap});
        if (m_zoomMode == ZoomMode::FitPage) {
            applyFit();
        }
    } else {
        showRow(row, false);
    }
    noteCurrentPage();
}

void CanvasController::goToDestination(const Destination& destination) {
    if (m_layout.rowCount() == 0 || !destination.valid()) {
        return;
    }
    const quint32 page = std::min(destination.page, m_layout.pageCount() - 1);
    const double height = m_session->pageSize(page).height();
    // Only an upright page of known height can place a point; otherwise the page is shown.
    const bool placeable = m_mode.rotation == 0 && height > 0.0;
    // Points down from the top of the page, for a y measured up from its bottom.
    const auto fromTop = [height](double y) { return std::max(0.0, height - y); };
    switch (destination.fit) {
    case FitKind::Xyz: {
        const bool zoomed = Destination::has(destination.zoom) && destination.zoom > 0.0;
        const double zoom = zoomed ? std::clamp(destination.zoom, kMinZoom, kMaxZoom) : m_zoom;
        if (placeable && Destination::has(destination.top)) {
            restoreView({page, fromTop(destination.top)}, zoom);
        } else {
            if (zoomed) {
                setZoom(zoom, QPointF(0.0, 0.0));
            }
            goToPage(page);
        }
        break;
    }
    case FitKind::FitH:
    case FitKind::FitBH:
        fitWidth();
        goToPage(page);
        if (placeable && Destination::has(destination.top)) {
            setScrollPosition(
                {m_scroll.x(), m_layout.yOf({page, fromTop(destination.top)}, m_zoom)});
        }
        break;
    case FitKind::FitR: {
        const double width = destination.right - destination.left;
        const double rectHeight = destination.top - destination.bottom;
        if (placeable && width > 0.0 && rectHeight > 0.0 && !m_viewport.isEmpty()) {
            const double gap = PageLayout::kGap;
            const double zoom = std::clamp(std::min((m_viewport.width() - 2.0 * gap) / width,
                                                    (m_viewport.height() - 2.0 * gap) / rectHeight),
                                           kMinZoom, kMaxZoom);
            restoreView({page, fromTop(destination.top)}, zoom);
            break;
        }
        fitPage();
        goToPage(page);
        break;
    }
    default: // Fit, FitB, FitV, FitBV, and anything a later protocol adds
        fitPage();
        goToPage(page);
        break;
    }
    noteCurrentPage();
}

void CanvasController::revealRect(quint32 page, const QRectF& points) {
    if (m_layout.rowCount() == 0 || m_viewport.isEmpty()) {
        return;
    }
    page = std::min(page, m_layout.pageCount() - 1);
    if (!m_mode.continuous) {
        const quint32 row = m_layout.rowOf(page);
        if (row != std::min(m_row, m_layout.rowCount() - 1)) {
            showRow(row, false);
        }
    }
    if (!points.isValid()) {
        if (m_mode.continuous) {
            goToPage(page);
        }
        return;
    }
    const PageDraw draw{page, pageScreenRect(page), m_layout.pageSize(page), m_mode.rotation,
                        m_zoom};
    const QRectF target = draw.map(points);
    const QRectF view(QPointF(0.0, 0.0), QSizeF(m_viewport));
    // Well inside: a margin of a fifth of the window on each side, so that a hit at the very edge
    // is brought towards the middle.
    const QRectF comfortable = view.adjusted(view.width() / 5.0, view.height() / 5.0,
                                             -view.width() / 5.0, -view.height() / 5.0);
    if (comfortable.contains(target)) {
        return;
    }
    setScrollPosition(m_scroll + target.center() - view.center());
}

void CanvasController::nextPage() {
    if (m_layout.rowCount() == 0) {
        return;
    }
    const quint32 row = m_layout.rowOf(currentPage());
    if (row + 1 < m_layout.rowCount()) {
        goToPage(m_layout.firstPageOf(row + 1));
    }
}

void CanvasController::previousPage() {
    if (m_layout.rowCount() == 0) {
        return;
    }
    const quint32 row = m_layout.rowOf(currentPage());
    if (row > 0) {
        goToPage(m_layout.firstPageOf(row - 1));
    }
}

// ---- engine events ----

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
    m_row = m_layout.rowCount() == 0 ? 0 : std::min(m_row, m_layout.rowCount() - 1);
    m_inFlight.clear();
    m_failed.clear();
    applyFit();
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

// ---- what is on screen ----

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
    const QRectF region(0.0, top, m_viewport.width(), bottom - top);

    const PageLayout::Range rows = visibleRows(top, bottom);
    for (quint32 r = 0; r < rows.count; ++r) {
        const quint32 row = rows.first + r;
        const quint32 first = m_layout.firstPageOf(row);
        for (quint32 i = 0; i < m_layout.pagesInRow(row); ++i) {
            const quint32 page = first + i;
            const QSizeF file = m_layout.pageSize(page);
            const QRectF shownPage = pageScreenRect(page);
            const QRectF visible = shownPage.intersected(region);
            if (visible.isEmpty()) {
                continue;
            }
            // The visible part of the page, in points of the page as it is in the file.
            const QRectF wanted = PageLayout::toPage(visible.translated(-shownPage.topLeft()), file,
                                                     m_mode.rotation, zoom)
                                      .intersected(QRectF(QPointF(0.0, 0.0), file));
            if (wanted.isEmpty()) {
                continue;
            }
            const auto firstX = static_cast<quint32>(std::floor(wanted.left() / tilePts));
            const auto lastX =
                static_cast<quint32>(std::max(1.0, std::ceil(wanted.right() / tilePts))) - 1;
            const auto firstY = static_cast<quint32>(std::floor(wanted.top() / tilePts));
            const auto lastY =
                static_cast<quint32>(std::max(1.0, std::ceil(wanted.bottom() / tilePts))) - 1;
            for (quint32 ty = firstY; ty <= lastY; ++ty) {
                for (quint32 tx = firstX; tx <= lastX; ++tx) {
                    if (tiles.size() >= kMaxTilesPerPass) {
                        return tiles;
                    }
                    const QRectF all(tx * tilePts, ty * tilePts, tilePts, tilePts);
                    const QRectF shown = all.intersected(QRectF(QPointF(0.0, 0.0), file));
                    if (shown.isEmpty()) {
                        continue;
                    }
                    TileDraw tile;
                    tile.page = page;
                    tile.x = tx;
                    tile.y = ty;
                    tile.bucket = bucket;
                    tile.scale = exact;
                    tile.rotation = m_mode.rotation;
                    tile.dest = PageLayout::toScreen(shown, file, m_mode.rotation, zoom)
                                    .translated(shownPage.topLeft());
                    tile.uv =
                        QRectF((shown.x() - all.x()) / tilePts, (shown.y() - all.y()) / tilePts,
                               shown.width() / tilePts, shown.height() / tilePts);
                    tiles.append(tile);
                }
            }
        }
    }
    return tiles;
}

QVector<PageDraw> CanvasController::visiblePages() const {
    QVector<PageDraw> pages;
    if (m_layout.pageCount() == 0 || m_viewport.isEmpty()) {
        return pages;
    }
    const PageLayout::Range rows = visibleRows(0.0, m_viewport.height());
    for (quint32 r = 0; r < rows.count; ++r) {
        const quint32 row = rows.first + r;
        const quint32 first = m_layout.firstPageOf(row);
        for (quint32 i = 0; i < m_layout.pagesInRow(row); ++i) {
            const quint32 page = first + i;
            pages.append(
                {page, pageScreenRect(page), m_layout.pageSize(page), m_mode.rotation, m_zoom});
        }
    }
    return pages;
}

Frame CanvasController::frame() const {
    Frame frame;
    frame.pages = visiblePages();
    if (!frame.pages.isEmpty()) {
        frame.tiles = tilesIn(0.0, m_viewport.height());
    }
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
