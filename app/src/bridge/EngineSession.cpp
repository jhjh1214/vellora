#include "bridge/EngineSession.h"

#include <QDir>
#include <atomic>
#include <exception>

namespace vellora {

namespace {

// How often the event queue is drained. The queue is polled, not signalled, so this is the latency
// of an answer; it is far below a frame, and polling an empty queue is a lock and a swap.
constexpr int kPollIntervalMs = 8;

QString toQString(const rust::String& text) {
    return QString::fromUtf8(text.data(), static_cast<qsizetype>(text.size()));
}

Destination toDestination(const OutlineNode& node) {
    Destination destination;
    destination.page = node.page;
    destination.fit = node.fit;
    destination.left = static_cast<double>(node.left);
    destination.top = static_cast<double>(node.top);
    destination.right = static_cast<double>(node.right);
    destination.bottom = static_cast<double>(node.bottom);
    destination.zoom = static_cast<double>(node.zoom);
    return destination;
}

Link toLink(const LinkNode& node) {
    Link link;
    link.rect = QRectF(QPointF(static_cast<double>(node.left), static_cast<double>(node.top)),
                       QPointF(static_cast<double>(node.right), static_cast<double>(node.bottom)))
                    .normalized();
    link.kind = node.kind;
    link.named = node.named;
    link.text = toQString(node.text);
    link.destination = toDestination(node.destination);
    return link;
}

OutlineItem toItem(const OutlineNode& node) {
    OutlineItem item;
    item.id = node.id;
    item.title = toQString(node.title);
    item.destination = toDestination(node);
    item.hasChildren = node.has_children;
    item.open = node.open;
    item.bold = node.bold;
    item.italic = node.italic;
    return item;
}

std::atomic<int> g_liveSessions{0};

} // namespace

int EngineSession::liveCount() {
    return g_liveSessions.load();
}

EngineSession::EngineSession(QObject* parent) : QObject(parent) {
    ++g_liveSessions;
    m_timer.setInterval(kPollIntervalMs);
    connect(&m_timer, &QTimer::timeout, this, &EngineSession::pollNow);
}

EngineSession::~EngineSession() {
    close();
    --g_liveSessions;
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

QString EngineSession::submitPassword(const QString& password) {
    if (!m_client) {
        return tr("No document is open.");
    }
    QByteArray utf8 = password.toUtf8();
    QString error;
    try {
        (*m_client)->submit_password(rust::Str(utf8.constData(), static_cast<size_t>(utf8.size())));
    } catch (const std::exception& failure) {
        error = QString::fromUtf8(failure.what());
    }
    utf8.fill('\0');
    return error;
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

TileTicket EngineSession::requestThumbnail(quint32 page, float zoom, quint32 width,
                                           quint32 height) {
    if (!m_client) {
        return TileTicket{TileState::Full, 0};
    }
    try {
        return (*m_client)->request_thumbnail(page, zoom, width, height);
    } catch (const std::exception&) {
        // The engine is down (it restarts), the size is invalid or thumbnails are off.
        return TileTicket{TileState::Full, 0};
    }
}

bool EngineSession::readThumbnail(quint32 page, float zoom, quint32 width, quint32 height,
                                  QImage& out) {
    if (!m_client || width == 0 || height == 0 || width > tilePixels() || height > tilePixels()) {
        return false;
    }
    // BGRx with tight rows, which is what RGB32 is in memory on little-endian machines.
    if (out.format() != QImage::Format_RGB32 || out.width() != static_cast<int>(width) ||
        out.height() != static_cast<int>(height)) {
        out = QImage(static_cast<int>(width), static_cast<int>(height), QImage::Format_RGB32);
    }
    if (out.isNull() || out.bytesPerLine() != static_cast<qsizetype>(width) * 4) {
        return false;
    }
    try {
        return (*m_client)->read_thumbnail(
            page, zoom, rust::Slice<uint8_t>(out.bits(), static_cast<size_t>(out.sizeInBytes())));
    } catch (const std::exception&) {
        return false;
    }
}

quint64 EngineSession::requestOutline(std::optional<quint32> parent, std::optional<quint32> after,
                                      quint32 already, quint32 limit) {
    if (!m_client) {
        return 0;
    }
    try {
        return (*m_client)->request_outline(parent.value_or(0), parent.has_value(),
                                            after.value_or(0), after.has_value(), already, limit);
    } catch (const std::exception&) {
        return 0;
    }
}

quint64 EngineSession::requestLinks(quint32 page, quint32 skip, quint32 limit) {
    if (!m_client) {
        return 0;
    }
    try {
        return (*m_client)->request_links(page, skip, limit);
    } catch (const std::exception&) {
        return 0;
    }
}

quint64 EngineSession::requestOutlinePath(quint32 page) {
    if (!m_client) {
        return 0;
    }
    try {
        return (*m_client)->request_outline_path(page);
    } catch (const std::exception&) {
        return 0;
    }
}

quint64 EngineSession::requestPageLabels(quint32 first, quint32 count) {
    if (!m_client) {
        return 0;
    }
    try {
        return (*m_client)->request_page_labels(first, count);
    } catch (const std::exception&) {
        return 0;
    }
}

quint64 EngineSession::findPageLabel(const QString& text) {
    if (!m_client) {
        return 0;
    }
    try {
        const QByteArray utf8 = text.toUtf8();
        return (*m_client)->find_page_label(
            rust::Str(utf8.constData(), static_cast<size_t>(utf8.size())));
    } catch (const std::exception&) {
        return 0;
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
    case EventKind::Opened: {
        m_pageCount = event.page_count;
        QStringList repairs;
        repairs.reserve(static_cast<qsizetype>(event.repairs.size()));
        for (const RepairNote& repair : event.repairs) {
            repairs.append(toQString(repair.message));
        }
        emit opened(event.page_count, repairs);
        break;
    }
    case EventKind::TileReady:
        emit tileReady(event.request);
        break;
    case EventKind::RequestFailed:
        if (!event.has_request && (event.failure == FailureKind::PasswordRequired ||
                                   event.failure == FailureKind::WrongPassword)) {
            emit passwordRequested(event.failure == FailureKind::WrongPassword);
            break;
        }
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
    case EventKind::Outline: {
        QList<OutlineItem> items;
        items.reserve(static_cast<qsizetype>(event.outline.size()));
        for (const OutlineNode& node : event.outline) {
            items.append(toItem(node));
        }
        emit outlineReady(event.request, items, event.more);
        break;
    }
    case EventKind::OutlinePath: {
        QList<quint32> path;
        path.reserve(static_cast<qsizetype>(event.path.size()));
        for (const uint32_t id : event.path) {
            path.append(id);
        }
        emit outlinePathReady(event.request, path);
        break;
    }
    case EventKind::PageLabels: {
        QStringList labels;
        labels.reserve(static_cast<qsizetype>(event.labels.size()));
        for (const rust::String& label : event.labels) {
            labels.append(toQString(label));
        }
        emit pageLabelsReady(event.request, event.first, event.labels_defined, labels);
        break;
    }
    case EventKind::PageFound:
        emit pageFound(event.request, event.found, event.found_page);
        break;
    case EventKind::Links: {
        QList<Link> links;
        links.reserve(static_cast<qsizetype>(event.links.size()));
        for (const LinkNode& node : event.links) {
            links.append(toLink(node));
        }
        emit linksReady(event.request, event.links_page, links, event.more);
        break;
    }
    case EventKind::TextPage:
        break; // the shell asks for text in task 14b
    }
}

} // namespace vellora
