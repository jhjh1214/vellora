#include "bridge/EngineSession.h"

#include <QDir>
#include <exception>

namespace vellora {

namespace {

// How often the event queue is drained. The queue is polled, not signalled, so this is the latency
// of an answer; it is far below a frame, and polling an empty queue is a lock and a swap.
constexpr int kPollIntervalMs = 8;

QString toQString(const rust::String& text) {
    return QString::fromUtf8(text.data(), static_cast<qsizetype>(text.size()));
}

} // namespace

EngineSession::EngineSession(QObject* parent) : QObject(parent) {
    m_timer.setInterval(kPollIntervalMs);
    connect(&m_timer, &QTimer::timeout, this, &EngineSession::pollNow);
}

EngineSession::~EngineSession() {
    close();
}

QString EngineSession::open(const QString& path) {
    close();
    try {
        const QByteArray utf8 = QDir::toNativeSeparators(path).toUtf8();
        m_client.emplace(
            vellora::open(rust::Str(utf8.constData(), static_cast<size_t>(utf8.size()))));
    } catch (const std::exception& error) {
        m_client.reset();
        return QString::fromUtf8(error.what());
    }
    m_timer.start();
    return {};
}

void EngineSession::close() {
    m_timer.stop();
    if (m_client) {
        (*m_client)->close();
        m_client.reset();
    }
    m_pageCount = 0;
}

QSizeF EngineSession::pageSize(quint32 page) const {
    if (!m_client) {
        return {};
    }
    const PageExtent extent = (*m_client)->page_size(page);
    if (extent.width <= 0.0F || extent.height <= 0.0F) {
        return {};
    }
    return {extent.width, extent.height};
}

quint32 EngineSession::tilePixels() {
    return vellora::tile_pixels();
}

float EngineSession::bucketScale(float zoom) {
    return vellora::bucket_scale(zoom);
}

quint32 EngineSession::slotBytes() const {
    return m_client ? (*m_client)->slot_bytes() : 0;
}

TileTicket EngineSession::requestTile(quint32 page, float zoom, quint32 x, quint32 y,
                                      TilePriority priority) {
    if (!m_client) {
        return TileTicket{TileState::Full, 0};
    }
    try {
        return (*m_client)->request_tile(page, zoom, x, y, priority);
    } catch (const std::exception&) {
        // The engine is down (it restarts) or the tile is invalid: the caller asks again.
        return TileTicket{TileState::Full, 0};
    }
}

bool EngineSession::readTile(quint32 page, float zoom, quint32 x, quint32 y, QByteArray& out) {
    if (!m_client) {
        return false;
    }
    out.resize(static_cast<qsizetype>(slotBytes()));
    try {
        return (*m_client)->read_tile(page, zoom, x, y,
                                      rust::Slice<uint8_t>(reinterpret_cast<uint8_t*>(out.data()),
                                                           static_cast<size_t>(out.size())));
    } catch (const std::exception&) {
        return false;
    }
}

void EngineSession::invalidatePage(quint32 page) {
    if (m_client) {
        (*m_client)->invalidate_page(page);
    }
}

bool EngineSession::cancel(quint64 request) {
    return m_client && (*m_client)->cancel(request);
}

void EngineSession::pollNow() {
    if (!m_client) {
        return;
    }
    // A signal handler may close the session; stop as soon as it has.
    for (const EngineEvent& event : (*m_client)->poll_events()) {
        dispatch(event);
        if (!m_client) {
            return;
        }
    }
}

void EngineSession::dispatch(const EngineEvent& event) {
    switch (event.kind) {
    case EventKind::Opened:
        m_pageCount = event.page_count;
        emit opened(event.page_count, event.repaired);
        break;
    case EventKind::TileReady:
        emit tileReady(event.request);
        break;
    case EventKind::RequestFailed:
        emit requestFailed(event.has_request ? event.request : 0, toQString(event.message));
        break;
    case EventKind::EngineCrashed: {
        QList<quint64> lost;
        lost.reserve(static_cast<qsizetype>(event.lost.size()));
        for (const uint64_t id : event.lost) {
            lost.append(id);
        }
        emit engineCrashed(toQString(event.message), event.will_restart, lost);
        break;
    }
    case EventKind::EngineRestarted:
        emit engineRestarted();
        break;
    case EventKind::Failed:
        emit failed(toQString(event.message));
        break;
    case EventKind::EngineTimeout:
        emit engineTimedOut(event.timeout, toQString(event.message));
        break;
    case EventKind::DocumentChanged:
        emit documentChanged(event.file_replaced);
        break;
    }
}

} // namespace vellora
