#include "canvas/LinkLayer.h"

#include <QMetaObject>
#include <algorithm>
#include <optional>

namespace vellora {

LinkLayer::LinkLayer(EngineSession* session, CanvasController* controller, QObject* parent)
    : QObject(parent), m_session(session), m_controller(controller) {
    connect(m_session, &EngineSession::opened, this, &LinkLayer::onOpened);
    connect(m_session, &EngineSession::linksReady, this, &LinkLayer::onLinks);
    connect(m_session, &EngineSession::requestFailed, this,
            [this](quint64 request, const QString&) { onRequestFailed(request); });
    connect(m_session, &EngineSession::engineCrashed, this, &LinkLayer::onEngineCrashed);
    connect(m_controller, &CanvasController::viewChanged, this, [this] { viewMoved(); });
    connect(m_controller, &CanvasController::contentChanged, this, [this] { viewMoved(); });
}

void LinkLayer::reset() {
    m_pages.clear();
    m_requests.clear();
    m_visible.clear();
    m_dirty = true;
}

void LinkLayer::viewMoved() {
    m_dirty = true;
    // Coalesced: a fling moves the view many times a frame.
    if (!m_queued) {
        m_queued = true;
        QMetaObject::invokeMethod(this, &LinkLayer::wantVisible, Qt::QueuedConnection);
    }
}

const QVector<PageDraw>& LinkLayer::visiblePages() const {
    if (m_dirty) {
        m_visible = m_controller->frame().pages;
        m_dirty = false;
    }
    return m_visible;
}

void LinkLayer::onOpened() {
    // Also the answer of a restarted engine: what was lost with the old one is asked for again.
    viewMoved();
}

void LinkLayer::wantVisible() {
    m_queued = false;
    const QVector<PageDraw>& visible = visiblePages();
    for (const PageDraw& page : visible) {
        if (page.page < m_controller->pageCount() && !m_pages.contains(page.page)) {
            askFor(page.page);
        }
    }
    evict(visible);
}

void LinkLayer::askFor(quint32 page) {
    PageLinks& entry = m_pages[page];
    if (entry.request != 0 || entry.complete) {
        return;
    }
    const quint64 id =
        m_session->requestLinks(page, static_cast<quint32>(entry.links.size()), kPerRequest);
    if (id == 0) {
        m_pages.remove(page); // the engine is down: asked again when it is back
        return;
    }
    entry.request = id;
    m_requests.insert(id, page);
}

void LinkLayer::evict(const QVector<PageDraw>& visible) {
    if (m_pages.size() <= kMaxCachedPages) {
        return;
    }
    QList<quint32> onScreen;
    for (const PageDraw& page : visible) {
        onScreen.append(page.page);
    }
    // Pages that are neither on screen nor waiting for an answer, furthest from the view first.
    const quint32 centre = onScreen.isEmpty() ? 0 : onScreen.first();
    QList<quint32> candidates;
    for (auto it = m_pages.cbegin(); it != m_pages.cend(); ++it) {
        if (!onScreen.contains(it.key()) && it->request == 0) {
            candidates.append(it.key());
        }
    }
    std::sort(candidates.begin(), candidates.end(), [centre](quint32 a, quint32 b) {
        const auto distance = [centre](quint32 p) { return p > centre ? p - centre : centre - p; };
        return distance(a) > distance(b);
    });
    for (const quint32 page : candidates) {
        if (m_pages.size() <= kMaxCachedPages) {
            break;
        }
        m_pages.remove(page);
    }
}

void LinkLayer::onLinks(quint64 request, quint32 page, const QList<Link>& links, bool more) {
    const auto asked = m_requests.constFind(request);
    if (asked == m_requests.constEnd()) {
        return; // not ours, or forgotten by reset
    }
    const quint32 expected = *asked;
    m_requests.remove(request);
    const auto found = m_pages.find(expected);
    if (found == m_pages.end() || expected != page) {
        return;
    }
    found->request = 0;
    const qsizetype room = std::max<qsizetype>(0, kMaxLinksPerPage - found->links.size());
    found->links.append(links.first(std::min(links.size(), room)));
    if (more && !links.isEmpty() && found->links.size() < kMaxLinksPerPage) {
        askFor(expected);
    } else {
        found->complete = true;
    }
    m_dirty = true; // nothing moved, but the links on screen may have
    emit linksChanged(expected);
}

void LinkLayer::onRequestFailed(quint64 request) {
    const auto asked = m_requests.constFind(request);
    if (asked == m_requests.constEnd()) {
        return;
    }
    const quint32 page = *asked;
    m_requests.remove(request);
    const auto found = m_pages.find(page);
    if (found != m_pages.end()) {
        // A page whose links cannot be read has none; it is not asked for again.
        found->request = 0;
        found->complete = true;
    }
}

void LinkLayer::onEngineCrashed(const QString&, bool, const QList<quint64>& lost) {
    for (const quint64 id : lost) {
        const auto asked = m_requests.constFind(id);
        if (asked != m_requests.constEnd()) {
            m_pages.remove(*asked); // asked for again once the new engine has the document
            m_requests.remove(id);
        }
    }
}

QList<Link> LinkLayer::linksOf(quint32 page) const {
    const auto found = m_pages.constFind(page);
    return found == m_pages.constEnd() ? QList<Link>() : found->links;
}

bool LinkLayer::isComplete(quint32 page) const {
    const auto found = m_pages.constFind(page);
    return found != m_pages.constEnd() && found->complete;
}

LinkLayer::Hit LinkLayer::hitTest(QPointF viewportPos) const {
    const QVector<PageDraw>& visible = visiblePages();
    for (const PageDraw& page : visible) {
        if (!page.rect.contains(viewportPos)) {
            continue;
        }
        const auto found = m_pages.constFind(page.page);
        if (found == m_pages.constEnd()) {
            return {};
        }
        // The last link listed is drawn on top, so it is tried first.
        for (qsizetype i = found->links.size(); i-- > 0;) {
            const Link& link = found->links.at(i);
            if (page.map(link.rect).contains(viewportPos)) {
                return {page.page, &link};
            }
        }
        return {};
    }
    return {};
}

} // namespace vellora
