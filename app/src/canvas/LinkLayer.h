// The links of the pages on screen, and which one is under the pointer. The engine is asked for
// the links of each page that comes into view (a window of 256 at a time, up to
// `kMaxLinksPerPage`), and the answers are kept for the pages that stay near the view. The layer
// only knows where links are and what they say; what activating one does is decided elsewhere
// (`LinkActions`, `DocumentTab`).
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"

#include <QHash>
#include <QList>
#include <QObject>
#include <QPointF>
#include <QVector>

namespace vellora {

class LinkLayer : public QObject {
    Q_OBJECT

public:
    // Links asked for in one request (the protocol's most).
    static constexpr quint32 kPerRequest = 256;
    // Links kept for one page; a page with more has the rest left out.
    static constexpr int kMaxLinksPerPage = 4096;
    // Pages whose links are kept; the ones furthest from the view are dropped beyond this.
    static constexpr int kMaxCachedPages = 64;

    // Neither is owned; both must outlive the layer.
    LinkLayer(EngineSession* session, CanvasController* controller, QObject* parent = nullptr);

    // Forgets the previous document's links. Call before the session opens another file.
    void reset();

    // The link under `viewportPos` (logical pixels of the canvas) and its page; the topmost of
    // overlapping links is the last one listed on the page, as PDF viewers do.
    struct Hit {
        quint32 page = 0;
        const Link* link = nullptr; // valid until the next event is processed
    };
    Hit hitTest(QPointF viewportPos) const;

    // The links of `page` read so far, and whether all of them were.
    QList<Link> linksOf(quint32 page) const;
    bool isComplete(quint32 page) const;
    int requestsInFlight() const { return static_cast<int>(m_requests.size()); }

signals:
    // More of the links of page have been read (the ones on screen may have changed).
    void linksChanged(quint32 page);

private slots:
    void onOpened();
    void onLinks(quint64 request, quint32 page, const QList<vellora::Link>& links, bool more);
    void onRequestFailed(quint64 request);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void wantVisible();

private:
    struct PageLinks {
        QList<Link> links;
        bool complete = false;
        quint64 request = 0; // outstanding, or 0
    };

    void viewMoved();
    void askFor(quint32 page);
    void evict(const QVector<PageDraw>& visible);
    const QVector<PageDraw>& visiblePages() const;

    EngineSession* m_session;
    CanvasController* m_controller;
    QHash<quint32, PageLinks> m_pages;
    QHash<quint64, quint32> m_requests; // request id -> page
    // The pages on screen as last laid out; recomputed only after the view changed.
    mutable QVector<PageDraw> m_visible;
    mutable bool m_dirty = true;
    bool m_queued = false;
};

} // namespace vellora
