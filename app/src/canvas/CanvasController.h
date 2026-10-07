// The canvas's brain, without any drawing: which part of the column is on screen, which tiles that
// needs, and asking the engine for them (and withdrawing requests that scrolling made pointless).
// The widget that draws (CanvasWidget) and the scroll area around it only talk to this class, which
// is why it can be tested with a real engine and no GPU.
//
// Coordinates: "logical" pixels are Qt's (what mouse and widget sizes use). Tiles are rendered in
// device pixels at `zoom * devicePixelRatio`, bucketed (EngineSession::bucketScale), and drawn
// stretched by the small difference between the bucket and the exact scale.
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
    QRectF uv;          // the part of the 512 px tile that `dest` shows, 0..1
    TileId id() const { return {page, x, y, bucket}; }
};

struct PageDraw {
    quint32 page = 0;
    QRectF rect; // viewport logical pixels
};

struct Frame {
    QVector<PageDraw> pages;
    QVector<TileDraw> tiles; // only what is on screen
};

class CanvasController : public QObject {
    Q_OBJECT

public:
    static constexpr double kMinZoom = 0.1;
    static constexpr double kMaxZoom = 8.0;
    // Tiles requested beyond the viewport: one viewport above and below, at most this many.
    static constexpr int kMaxPrefetchTiles = 48;

    explicit CanvasController(EngineSession* session, QObject* parent = nullptr);

    // Forgets the view of the previous document (scroll, zoom, requests). Call before opening.
    void reset();

    void setViewportSize(QSize logicalSize);
    void setDevicePixelRatio(double ratio);
    QSize viewportSize() const { return m_viewport; }

    // Total size of the column in logical pixels at the current zoom (at least the viewport wide).
    QSizeF contentSize() const;
    QPointF scrollPosition() const { return m_scroll; }
    // Clamps to the content; emits `viewChanged` if anything moved.
    void setScrollPosition(QPointF position);

    double zoom() const { return m_zoom; }
    // Zooms by `factor`, keeping the document point under `anchor` (viewport pixels) in place.
    void zoomBy(double factor, QPointF anchor);
    void setZoom(double zoom, QPointF anchor);
    void actualSize();
    void fitWidth();

    quint32 pageCount() const { return m_layout.pageCount(); }
    // The page under the middle of the viewport.
    quint32 currentPage() const;
    const PageLayout& layout() const { return m_layout; }

    // What to draw now. Cheap: geometry only.
    Frame frame() const;

    // Tiles with an outstanding request (for tests and diagnostics).
    int tilesInFlight() const { return static_cast<int>(m_inFlight.size()); }

signals:
    // The column changed size (zoom, new document): the scroll bars need new ranges.
    void contentChanged();
    // Something visible changed: repaint.
    void viewChanged();
    void currentPageChanged(quint32 page, quint32 pageCount);
    void zoomChanged(double zoom);

private slots:
    void onOpened();
    void onTileReady(quint64 request);
    void onRequestFailed(quint64 request);
    void onEngineCrashed();
    void onEngineRestarted();

private:
    double contentWidth() const;
    double pageLeft(quint32 page) const; // viewport logical pixels
    QPointF clampedScroll(QPointF position) const;
    void applyZoom(double zoom, QPointF anchor);
    void rebuildLayout();
    // The tiles that cover viewport rows [top, bottom) (viewport logical pixels).
    QVector<TileDraw> tilesIn(double top, double bottom) const;
    void schedule();
    void noteCurrentPage();

    EngineSession* m_session;
    PageLayout m_layout;
    QSize m_viewport;
    double m_ratio = 1.0;
    double m_zoom = 1.0;
    QPointF m_scroll;
    quint32 m_reportedPage = std::numeric_limits<quint32>::max();

    QHash<quint64, TileId> m_inFlight;
    QSet<TileId> m_failed;
};

} // namespace vellora
