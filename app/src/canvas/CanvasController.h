// The canvas's brain, without any drawing: which part of the column is on screen, which tiles that
// needs, and asking the engine for them (and withdrawing requests that scrolling made pointless).
// The widget that draws (CanvasWidget) and the scroll area around it only talk to this class, which
// is why it can be tested with a real engine and no GPU.
//
// View modes: pages are laid out one or two a row (PageLayout), continuously or one row at a time;
// the whole view can be turned in quarter turns (never written to the file). Zoom is custom, or
// follows the window (fit width, fit page) until the user zooms by hand.
//
// Coordinates: "logical" pixels are Qt's (what mouse and widget sizes use). Tiles are rendered in
// device pixels at `zoom * devicePixelRatio`, bucketed (EngineSession::bucketScale), of the page as
// it is in the file; the widget turns them when it draws a turned view.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/PageLayout.h"

#include <QHash>
#include <QObject>
#include <QPointF>
#include <QRectF>
#include <QSet>
#include <QSize>
#include <QSizeF>
#include <QVector>
#include <limits>

namespace vellora {

// The identity of a tile: page, grid position and render scale. Two draws with the same id show
// the same pixels, so it also keys the GPU texture cache.
struct TileId {
    quint32 page = 0;
    quint32 x = 0;
    quint32 y = 0;
    int bucket = 0;
    friend bool operator==(const TileId&, const TileId&) = default;
    friend size_t qHash(const TileId& id, size_t seed = 0) noexcept {
        return qHashMulti(seed, id.page, id.x, id.y, id.bucket);
    }
};

// One tile as the canvas sees it: which tile, and where on screen it goes.
struct TileDraw {
    quint32 page = 0;
    quint32 x = 0; // tile column and row in the grid at this bucket
    quint32 y = 0;
    int bucket = 0;     // quarter-octave index of the render scale (part of the tile's identity)
    float scale = 1.0F; // what to pass to EngineSession (the exact device scale)
    QRectF dest;        // viewport logical pixels, clipped to the page
    QRectF uv; // the part of the 512 px tile that `dest` shows, 0..1, in the tile as rendered
    int rotation = 0; // quarter turns clockwise the tile is drawn with
    TileId id() const { return {page, x, y, bucket}; }
};

struct PageDraw {
    quint32 page = 0;
    QRectF rect; // viewport logical pixels, as shown (turned)
    // What the widget needs to place any part of the page: the page's size in the file (points),
    // the turn and the zoom.
    QSizeF filePoints;
    int rotation = 0;
    double zoom = 1.0;

    // The viewport rectangle that shows `points`, a rectangle of the page as it is in the file.
    QRectF map(const QRectF& points) const {
        return PageLayout::toScreen(points, filePoints, rotation, zoom).translated(rect.topLeft());
    }
};

struct Frame {
    QVector<PageDraw> pages;
    QVector<TileDraw> tiles; // only what is on screen
};

class CanvasController : public QObject {
    Q_OBJECT

public:
    static constexpr double kMinZoom = 0.1;
    // 6400%, the largest scale the protocol renders (`vellora_ipc::MAX_TILE_SCALE`).
    static constexpr double kMaxZoom = 64.0;
    // Tiles requested beyond the viewport: one viewport above and below, at most this many.
    static constexpr int kMaxPrefetchTiles = 48;

    // How the pages are arranged.
    struct ViewMode {
        // All rows in one scrolling column, or one row at a time.
        bool continuous = true;
        PageLayout::Spread spread = PageLayout::Spread::One;
        // Quarter turns clockwise, 0..3.
        int rotation = 0;
        friend bool operator==(const ViewMode&, const ViewMode&) = default;
    };
    // What the zoom follows.
    enum class ZoomMode { Custom, FitWidth, FitPage };

    // The zoom levels the menu offers: 25% to 6400%.
    static const QVector<double>& zoomPresets();

    explicit CanvasController(EngineSession* session, QObject* parent = nullptr);

    // Forgets the view of the previous document (scroll, zoom, requests). Call before opening. The
    // view mode is kept: it is the user's choice, not the document's.
    void reset();

    void setViewportSize(QSize logicalSize);
    void setDevicePixelRatio(double ratio);
    QSize viewportSize() const { return m_viewport; }

    // ---- view mode ----
    ViewMode viewMode() const { return m_mode; }
    void setViewMode(ViewMode mode);
    void setContinuous(bool continuous);
    void setSpread(PageLayout::Spread spread);
    // Turns the view by `quarterTurns` clockwise (negative: anticlockwise).
    void rotateBy(int quarterTurns);
    void setRotation(int quarterTurns);

    // ---- scrolling ----
    // Total size of the column in logical pixels at the current zoom (at least the viewport wide).
    QSizeF contentSize() const;
    QPointF scrollPosition() const { return m_scroll; }
    // The scroll positions that are allowed: the whole column when continuous, the current row
    // when not.
    struct VerticalRange {
        double min = 0.0;
        double max = 0.0;
    };
    VerticalRange verticalRange() const;
    // Clamps to the allowed range; emits `viewChanged` if anything moved.
    void setScrollPosition(QPointF position);
    // Scrolls by `delta` logical pixels. Without continuous scrolling, going past the end of the
    // row turns to the next row (or the previous one), the way turning a page does.
    void scrollBy(QPointF delta);

    // ---- zoom ----
    double zoom() const { return m_zoom; }
    ZoomMode zoomMode() const { return m_zoomMode; }
    // Zooms by `factor`, keeping the document point under `anchor` (viewport pixels) in place.
    void zoomBy(double factor, QPointF anchor);
    void setZoom(double zoom, QPointF anchor);
    void actualSize();
    // Fits the widest row to the window width, or the current row to the window; the zoom keeps
    // following the window until it is set by hand.
    void fitWidth();
    void fitPage();
    // Zooms so that `rect` (viewport pixels) fills the window, and centres it.
    void zoomToRect(const QRectF& rect);

    // ---- navigation ----
    // The place at the top edge of the viewport, and putting the view back there at a zoom (for
    // restoring a saved view once the layout is known).
    PageLayout::Anchor topAnchor() const { return m_layout.anchorAt(m_scroll.y(), m_zoom); }
    void restoreView(PageLayout::Anchor anchor, double zoom);

    quint32 pageCount() const { return m_layout.pageCount(); }
    // The page under the middle of the viewport (the first page of its row), or, when the view
    // shows one row at a time, of the row shown.
    quint32 currentPage() const;
    // Scrolls so that the row of `page` starts at the top of the window; when the view shows one
    // row at a time, it shows that row.
    void goToPage(quint32 page);
    // Follows a destination of the document: the page, and how it is shown. `Xyz` puts the point
    // at the top edge of the window (at its zoom, if it has one); `Fit` shows the whole page,
    // `FitH` its width, `FitR` fills the window with the rectangle. Coordinates are measured up
    // from the bottom of the page as shown, which holds for the usual page whose box starts at
    // the origin; in a turned view only the page is followed. The `B` variants are treated as the
    // plain ones (the content box is not known here).
    void goToDestination(const Destination& destination);
    // Brings `points` (a rectangle of `page`, in points of the page as shown) into view: nothing
    // moves if it is already well inside the window, else the view scrolls to put its centre in the
    // middle (when the view shows one row at a time, the row of `page` is shown first). The zoom
    // does not change.
    void revealRect(quint32 page, const QRectF& points);
    void nextPage();
    void previousPage();
    const PageLayout& layout() const { return m_layout; }

    // What to draw now. Cheap: geometry only.
    Frame frame() const;
    // Just the pages on screen, without the tiles: where each is and how it is turned and scaled.
    QVector<PageDraw> visiblePages() const;

    // Tiles with an outstanding request (for tests and diagnostics).
    int tilesInFlight() const { return static_cast<int>(m_inFlight.size()); }

signals:
    // The column changed size (zoom, new document, view mode): the scroll bars need new ranges.
    void contentChanged();
    // Something visible changed: repaint.
    void viewChanged();
    void currentPageChanged(quint32 page, quint32 pageCount);
    void zoomChanged(double zoom);
    // The arrangement or the turn changed.
    void viewModeChanged(vellora::CanvasController::ViewMode mode);
    void zoomModeChanged(vellora::CanvasController::ZoomMode mode);

private slots:
    void onOpened();
    void onTileReady(quint64 request);
    void onRequestFailed(quint64 request);
    void onEngineCrashed();
    void onEngineRestarted();

private:
    double contentWidth() const;
    // Where `page` is in the viewport (logical pixels), as shown.
    QRectF pageScreenRect(quint32 page) const;
    // The rows that are on screen (all that intersect the viewport when continuous, else the row
    // shown) and the pages in them.
    PageLayout::Range visibleRows(double top, double bottom) const;
    QPointF clampedScroll(QPointF position) const;
    void applyZoom(double zoom, QPointF anchor);
    // Re-fits the zoom if it follows the window.
    void applyFit();
    void applyArrangement();
    void rebuildLayout();
    void showRow(quint32 row, bool fromBottom);
    // The tiles that cover viewport rows [top, bottom) (viewport logical pixels).
    QVector<TileDraw> tilesIn(double top, double bottom) const;
    void schedule();
    void noteCurrentPage();
    void setZoomMode(ZoomMode mode);

    EngineSession* m_session;
    PageLayout m_layout;
    ViewMode m_mode;
    ZoomMode m_zoomMode = ZoomMode::Custom;
    QSize m_viewport;
    double m_ratio = 1.0;
    double m_zoom = 1.0;
    QPointF m_scroll;
    // The row shown when the view shows one row at a time.
    quint32 m_row = 0;
    quint32 m_reportedPage = std::numeric_limits<quint32>::max();

    QHash<quint64, TileId> m_inFlight;
    QSet<TileId> m_failed;
};

} // namespace vellora