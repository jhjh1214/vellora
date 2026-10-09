#include "sidebar/OutlineView.h"

#include <QFont>
#include <QPaintEvent>
#include <QPainter>
#include <QShowEvent>
#include <algorithm>

namespace vellora {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kPlaceholderRole = Qt::UserRole + 1;

bool isPlaceholder(const QTreeWidgetItem* item) {
    return item->data(0, kPlaceholderRole).toBool();
}

} // namespace

OutlineView::OutlineView(EngineSession* session, CanvasController* controller, QWidget* parent)
    : QTreeWidget(parent), m_session(session), m_controller(controller) {
    setHeaderHidden(true);
    setUniformRowHeights(true);
    setTextElideMode(Qt::ElideRight);
    setAccessibleName(tr("Document outline"));
    setEditTriggers(QAbstractItemView::NoEditTriggers);
    m_followTimer.setSingleShot(true);
    m_followTimer.setInterval(kFollowDelayMs);
    connect(&m_followTimer, &QTimer::timeout, this, &OutlineView::followNow);

    connect(m_session, &EngineSession::opened, this, &OutlineView::onOpened);
    connect(m_session, &EngineSession::outlineReady, this, &OutlineView::onOutline);
    connect(m_session, &EngineSession::outlinePathReady, this, &OutlineView::onOutlinePath);
    connect(m_session, &EngineSession::requestFailed, this,
            [this](quint64 request, const QString&) { onRequestFailed(request); });
    connect(m_session, &EngineSession::engineCrashed, this, &OutlineView::onEngineCrashed);
    connect(this, &QTreeWidget::itemExpanded, this, &OutlineView::onItemExpanded);
    connect(this, &QTreeWidget::itemClicked, this,
            [this](QTreeWidgetItem* item) { onItemActivated(item); });
    connect(this, &QTreeWidget::itemActivated, this,
            [this](QTreeWidgetItem* item) { onItemActivated(item); });
    connect(m_controller, &CanvasController::currentPageChanged, this,
            [this](quint32 page) { onCurrentPageChanged(page); });
}

void OutlineView::forgetDocument() {
    m_followTimer.stop();
    clear();
    m_items.clear();
    m_destinations.clear();
    m_levels.clear();
    m_requests.clear();
    m_path.clear();
    m_pathRequest = 0;
    viewport()->update();
}

bool OutlineView::topLevelLoaded() const {
    const auto top = m_levels.constFind(kTop);
    return top != m_levels.constEnd() && top->loaded;
}

quint32 OutlineView::idOf(const QTreeWidgetItem* item) const {
    return item != nullptr ? item->data(0, kIdRole).toUInt() : 0;
}

std::optional<Destination> OutlineView::destinationOf(const QTreeWidgetItem* item) const {
    if (item == nullptr || isPlaceholder(item)) {
        return std::nullopt;
    }
    const auto found = m_destinations.constFind(idOf(item));
    if (found == m_destinations.constEnd()) {
        return std::nullopt;
    }
    return *found;
}

// ---- reading levels ----

void OutlineView::onOpened() {
    // The first answer reads the top level; a restarted engine answers again, and then the levels
    // that were waiting (or lost with the old engine) are asked for again.
    if (!m_levels.contains(kTop)) {
        m_levels.insert(kTop, Level{invisibleRootItem(), std::nullopt, 0, 0, false, false});
    }
    for (auto it = m_levels.begin(); it != m_levels.end(); ++it) {
        const bool wanted = it.key() == kTop || (it->parent != nullptr && it->parent->isExpanded());
        if (wanted && it->request == 0 && !it->loaded && !it->failed) {
            requestLevel(it.key());
        }
    }
}

void OutlineView::requestLevel(qint64 key) {
    const auto found = m_levels.find(key);
    if (found == m_levels.end() || found->request != 0 || found->loaded) {
        return;
    }
    std::optional<quint32> parent;
    if (key != kTop) {
        parent = static_cast<quint32>(key);
    }
    const quint64 id = m_session->requestOutline(parent, found->after, found->already, kChunk);
    if (id != 0) {
        found->request = id;
        m_requests.insert(id, key);
    }
}

// The level a request was for, forgetting the request; empty if it was not one of ours.
std::optional<qint64> OutlineView::takeRequest(quint64 request) {
    const auto found = m_requests.constFind(request);
    if (found == m_requests.constEnd()) {
        return std::nullopt;
    }
    const qint64 key = *found;
    m_requests.remove(request);
    return key;
}

void OutlineView::addPlaceholder(QTreeWidgetItem* parent, const QString& text) {
    auto* line = new QTreeWidgetItem(parent);
    line->setText(0, text);
    line->setData(0, kPlaceholderRole, true);
    line->setFlags(Qt::ItemIsEnabled);
    QFont font = line->font(0);
    font.setItalic(true);
    line->setFont(0, font);
}

void OutlineView::removePlaceholders(QTreeWidgetItem* parent) {
    for (int i = parent->childCount() - 1; i >= 0; --i) {
        if (isPlaceholder(parent->child(i))) {
            delete parent->takeChild(i);
        }
    }
}

void OutlineView::onOutline(quint64 request, const QList<OutlineItem>& items, bool more) {
    const std::optional<qint64> asked = takeRequest(request);
    if (!asked) {
        return; // not ours, or forgotten by reset
    }
    const qint64 key = *asked;
    const auto found = m_levels.find(key);
    if (found == m_levels.end()) {
        return;
    }
    // A copy: adding the levels of the new items below rehashes `m_levels`, which would leave a
    // reference into it dangling. It is stored back before anything reads it again.
    Level level = *found;
    level.request = 0;
    QTreeWidgetItem* parent = level.parent;
    removePlaceholders(parent);
    const QString tipStyle = QStringLiteral("<p style='white-space:pre'>%1</p>");
    for (const OutlineItem& entry : items) {
        if (parent->childCount() >= kMaxItemsPerLevel) {
            break;
        }
        // The same item reached twice (a loop, or two parents sharing a child) is shown once.
        if (m_items.contains(entry.id)) {
            continue;
        }
        auto* item = new QTreeWidgetItem(parent);
        item->setText(0, entry.title.isEmpty() ? tr("(untitled)") : entry.title);
        item->setData(0, kIdRole, entry.id);
        item->setToolTip(0, tipStyle.arg(entry.title.toHtmlEscaped()));
        if (entry.bold || entry.italic) {
            QFont font = item->font(0);
            font.setBold(entry.bold);
            font.setItalic(entry.italic);
            item->setFont(0, font);
        }
        m_items.insert(entry.id, item);
        if (entry.destination.valid()) {
            m_destinations.insert(entry.id, entry.destination);
        }
        if (entry.hasChildren) {
            m_levels.insert(entry.id, Level{item, std::nullopt, 0, 0, false, false});
            addPlaceholder(item, tr("Loading…"));
        }
    }
    level.already += static_cast<quint32>(items.size());
    if (!items.isEmpty()) {
        level.after = items.last().id;
    }
    const bool continues = more && !items.isEmpty() && parent->childCount() < kMaxItemsPerLevel;
    if (!continues) {
        level.loaded = true;
        if (key != kTop && parent->childCount() == 0) {
            addPlaceholder(parent, tr("(empty)"));
        }
    }
    m_levels.insert(key, level);
    if (continues) {
        requestLevel(key);
    }
    if (key == kTop) {
        // The document asks for these to be open when it is opened.
        for (const OutlineItem& entry : items) {
            QTreeWidgetItem* item = m_items.value(entry.id, nullptr);
            if (entry.open && entry.hasChildren && item != nullptr && !item->isExpanded()) {
                item->setExpanded(true);
            }
        }
        viewport()->update();
    }
    advancePath();
}

void OutlineView::onRequestFailed(quint64 request) {
    if (request == m_pathRequest) {
        m_pathRequest = 0;
        return;
    }
    const std::optional<qint64> asked = takeRequest(request);
    if (!asked) {
        return;
    }
    const qint64 key = *asked;
    const auto found = m_levels.find(key);
    if (found == m_levels.end()) {
        return;
    }
    found->request = 0;
    found->failed = true;
    removePlaceholders(found->parent);
    addPlaceholder(found->parent,
                   key == kTop ? tr("The outline could not be read.") : tr("Could not be read"));
    viewport()->update();
}

void OutlineView::onEngineCrashed(const QString&, bool, const QList<quint64>& lost) {
    // What the old engine owed is not coming; `onOpened` asks again when the new one has the file.
    for (const quint64 request : lost) {
        if (const std::optional<qint64> asked = takeRequest(request)) {
            const auto found = m_levels.find(*asked);
            if (found != m_levels.end()) {
                found->request = 0;
            }
        }
        if (request == m_pathRequest) {
            m_pathRequest = 0;
        }
    }
}

void OutlineView::onItemExpanded(QTreeWidgetItem* item) {
    if (item != nullptr && !isPlaceholder(item)) {
        requestLevel(idOf(item));
    }
}

void OutlineView::onItemActivated(QTreeWidgetItem* item) {
    if (m_following) {
        return;
    }
    if (const std::optional<Destination> destination = destinationOf(item)) {
        emit destinationActivated(*destination);
    }
}

// ---- following the canvas ----

void OutlineView::onCurrentPageChanged(quint32 page) {
    m_followPage = page;
    m_followTimer.start();
}

void OutlineView::followNow() {
    followPage(m_followPage);
}

void OutlineView::followPage(quint32 page) {
    m_followPage = page;
    // Nothing to follow in a hidden tab (it follows when it is shown), before the top level is
    // read, or in a document without an outline.
    if (!isVisible() || !topLevelLoaded() || topLevelItemCount() == 0) {
        return;
    }
    if (m_pathRequest != 0) {
        m_session->cancel(m_pathRequest);
    }
    m_pathRequest = m_session->requestOutlinePath(page);
}

void OutlineView::showEvent(QShowEvent* event) {
    QTreeWidget::showEvent(event);
    m_followTimer.start();
}

void OutlineView::onOutlinePath(quint64 request, const QList<quint32>& path) {
    if (request == 0 || request != m_pathRequest) {
        return; // an older question, or one that was withdrawn
    }
    m_pathRequest = 0;
    m_path = path;
    advancePath();
}

void OutlineView::advancePath() {
    // Walks the path from the top, expanding as it goes. A level that has not been read yet is
    // asked for and the walk goes on when it arrives (`onOutline` calls this again).
    for (qsizetype i = 0; i < m_path.size(); ++i) {
        const quint32 id = m_path.at(i);
        QTreeWidgetItem* item = m_items.value(id, nullptr);
        if (item == nullptr) {
            const qint64 parentKey = i == 0 ? kTop : static_cast<qint64>(m_path.at(i - 1));
            const auto level = m_levels.constFind(parentKey);
            if (level != m_levels.constEnd() && !level->loaded && !level->failed) {
                requestLevel(parentKey); // a no-op while a request is out
                return;
            }
            m_path.clear(); // read, and the item is not there: it is beyond the level's cap
            return;
        }
        if (i + 1 < m_path.size()) {
            m_following = true;
            item->setExpanded(true);
            m_following = false;
            requestLevel(id);
            const auto level = m_levels.constFind(static_cast<qint64>(id));
            if (level != m_levels.constEnd() && !level->loaded && !level->failed) {
                return;
            }
            continue;
        }
        // The section's own item. Keep the reader's selection if it is on the same page: the
        // reader has just clicked it, and the tree should not move under their hand.
        const QTreeWidgetItem* current = currentItem();
        const auto have = destinationOf(current);
        const auto want = destinationOf(item);
        const bool sameSection = have && want && have->page == want->page;
        if (!sameSection) {
            m_following = true;
            setCurrentItem(item);
            scrollToItem(item);
            m_following = false;
        }
        m_path.clear();
    }
}

void OutlineView::paintEvent(QPaintEvent* event) {
    QTreeWidget::paintEvent(event);
    if (!isEmptyOutline()) {
        return;
    }
    QPainter painter(viewport());
    painter.setPen(palette().color(QPalette::Disabled, QPalette::Text));
    painter.drawText(viewport()->rect().adjusted(8, 8, -8, -8),
                     Qt::AlignHCenter | Qt::AlignTop | Qt::TextWordWrap,
                     tr("This document has no outline."));
}

} // namespace vellora
