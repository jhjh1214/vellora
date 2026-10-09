// The list of page thumbnails in the sidebar: one cell per page in a single column, with the page's
// label under it, drawn lazily. Only the cells on screen (and a few around them) are asked for, at
// the engine's lowest priority and into the thumbnail cache, which has its own budget: so a fling
// through thousands of pages neither delays the tiles of the page being read nor evicts them.
//
// The view follows the canvas (the row of its current page is highlighted and kept on screen) and
// drives it: a click, or an arrow key, Page Up/Down, Home or End, jumps the canvas to that page.
// Thumbnails show pages upright; turning the view does not turn them.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"

#include <QAbstractScrollArea>
#include <QHash>
#include <QSet>
#include <QStringList>
#include <QVector>

namespace vellora {

class ThumbnailView : public QAbstractScrollArea {
    Q_OBJECT

public:
    static constexpr int kMinWidth = 64;
    static constexpr int kMaxWidth = 256;
    static constexpr int kDefaultWidth = 120;
    // A page is shown at most this many times as tall as wide (the cell is not made taller for a
    // very long page; the page is made smaller).
    static constexpr double kMaxAspect = 2.0;
    // Thumbnails asked for at once: the cells on screen, then the nearest ones around them.
    static constexpr int kMaxWanted = 40;
    // Requests sent in one pass; the rest follow as answers come in.
    static constexpr int kMaxNewPerPass = 12;
    // The cells above and below the screen that are asked for once the ones on screen are.
    static constexpr int kMargin = 8;

    // Neither is owned; both must outlive the view.
    ThumbnailView(EngineSession* session, CanvasController* controller, QWidget* parent = nullptr);

    // Forgets the previous document (requests, selection, labels). Call before the session opens
    // another file.
    void reset();

    // ---- size ----
    // The width of a thumbnail in logical pixels, `kMinWidth` to `kMaxWidth`.
    int thumbnailWidth() const { return m_width; }
    void setThumbnailWidth(int width);

    // ---- pages ----
    quint32 pageCount() const { return m_pageCount; }
    // The page the canvas shows (its row is highlighted) and the one the keyboard moves from.
    quint32 highlightedPage() const { return m_highlight; }
    quint32 selectedPage() const { return m_selected; }
    // Whether the cell of `page` is drawn highlighted: it is in the row of the highlighted page
    // (both pages of a spread are).
    bool isHighlighted(quint32 page) const;
    // Text under the thumbnail of `page`: the page's label if `setPageLabels` gave one, else its
    // number. Plain text, never rich text: labels come from the document.
    QString labelFor(quint32 page) const;
    void setPageLabels(const QStringList& labels);

    // ---- geometry (viewport pixels) ----
    // The cell of `page` (the whole row, as wide as the viewport) and the thumbnail inside it.
    QRect cellRect(quint32 page) const;
    QRect imageRect(quint32 page) const;
    // The page whose cell contains `position`, or `pageCount()` if none does.
    quint32 pageAt(QPoint position) const;
    // The pages whose cells touch the viewport: first and one past the last (equal when none).
    struct Range {
        quint32 first = 0;
        quint32 end = 0;
    };
    Range visiblePages() const;
    // Total height of all cells.
    double contentHeight() const;
    // Scrolls just far enough to show the whole cell of `page`.
    void ensureVisible(quint32 page);

    // ---- keyboard and mouse ----
    // Highlights `page` and jumps the canvas to it.
    void goToPage(quint32 page);

    // ---- what is being asked of the engine ----
    int thumbnailsInFlight() const { return static_cast<int>(m_inFlight.size()); }
    // Requests sent since the document was opened (tests and diagnostics).
    quint64 requestsSent() const { return m_requestsSent; }
    // Whether the thumbnail of `page` is ready to be drawn at the current size.
    bool hasThumbnail(quint32 page);
    // Cells drawn since the view was created.
    quint64 paintCount() const { return m_paintCount; }

signals:
    void thumbnailWidthChanged(int width);
    // The time one repaint took, for the frame-time measurements.
    void paintTimed(double ms);

protected:
    void paintEvent(QPaintEvent* event) override;
    void resizeEvent(QResizeEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void wheelEvent(QWheelEvent* event) override;
    void scrollContentsBy(int dx, int dy) override;
    void focusInEvent(QFocusEvent* event) override;
    void focusOutEvent(QFocusEvent* event) override;

private slots:
    void onOpened();
    void onTileReady(quint64 request);
    void onRequestFailed(quint64 request);
    void onEngineCrashed();
    void onEngineRestarted();
    void onCurrentPageChanged(quint32 page);
    void schedule();

private:
    // What a thumbnail of one page is asked for with.
    struct Render {
        float zoom = 0.0F; // the bucketed scale, which is also the cache key
        quint32 width = 0; // pixels
        quint32 height = 0;
        bool valid() const { return zoom > 0.0F && width > 0 && height > 0; }
    };
    struct Key {
        quint32 page = 0;
        float zoom = 0.0F;
        friend bool operator==(const Key&, const Key&) = default;
        friend size_t qHash(const Key& key, size_t seed = 0) noexcept {
            return qHashMulti(seed, key.page, key.zoom);
        }
    };

    QSize imageSize(quint32 page) const;
    Render renderFor(quint32 page) const;
    void rebuildGeometry();
    void updateScrollBars();
    void scheduleSoon();
    void select(quint32 page);

    EngineSession* m_session;
    CanvasController* m_controller;
    int m_width = kDefaultWidth;
    quint32 m_pageCount = 0;
    // Top of each cell, in content pixels; one more entry than pages, the last being the total.
    QVector<double> m_tops;
    QStringList m_labels;
    quint32 m_highlight = 0;
    quint32 m_selected = 0;
    // A jump we made is in progress: the canvas's answer must not move the selection.
    bool m_jumping = false;
    bool m_scheduleQueued = false;

    QHash<quint64, Key> m_inFlight;
    QSet<Key> m_failed;
    quint64 m_requestsSent = 0;
    quint64 m_paintCount = 0;
};

} // namespace vellora
