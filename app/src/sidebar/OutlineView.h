// The document outline (bookmarks) as a tree in the sidebar. The tree is read from the engine a
// level at a time: the top level when the document opens, the children of an item when it is
// expanded, so a file with tens of thousands of bookmarks costs nothing until they are looked at.
// A level longer than the engine's page is read page after page, up to `kMaxItemsPerLevel`.
//
// The view follows the canvas: when the reader moves to another page, the engine is asked which
// outline items lead to the section the page is in, the tree expands to them and selects the last.
// Clicking an item (or pressing Enter on it) jumps the canvas to its destination, which the
// owner does through `destinationActivated` so that the jump is recorded in the history.
//
// Titles come from the document: they are plain text (a tree item never reads rich text), and the
// tool tip, which Qt would read as rich text if it looks like it, is escaped.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"

#include <QHash>
#include <QTimer>
#include <QTreeWidget>
#include <optional>

namespace vellora {

class OutlineView : public QTreeWidget {
    Q_OBJECT

public:
    // Items asked for in one request (the engine's page).
    static constexpr int kChunk = 128;
    // Most items shown under one parent; a level is not read further than this.
    static constexpr int kMaxItemsPerLevel = 4096;
    // The reader must stay on a page this long before the tree follows.
    static constexpr int kFollowDelayMs = 150;

    // Neither is owned; both must outlive the view.
    OutlineView(EngineSession* session, CanvasController* controller, QWidget* parent = nullptr);

    // Forgets the previous document. Call before the session opens another file. (Not called
    // reset(): that is a virtual of the item view, which clear() itself calls.)
    void forgetDocument();

    // ---- what has been read ----
    // Items in the tree, not counting the "Loading…" lines.
    int loadedItems() const { return static_cast<int>(m_items.size()); }
    // The top level has been read (the document may have no outline: then it is empty).
    bool topLevelLoaded() const;
    // The top level is read and has no items.
    bool isEmptyOutline() const { return topLevelLoaded() && topLevelItemCount() == 0; }
    // Requests sent to the engine and not answered yet.
    int requestsInFlight() const {
        return static_cast<int>(m_requests.size()) + (m_pathRequest != 0);
    }
    QTreeWidgetItem* itemForId(quint32 id) const { return m_items.value(id, nullptr); }
    quint32 idOf(const QTreeWidgetItem* item) const;
    // Where an item goes; empty for an item without a destination, or a "Loading…" line.
    std::optional<Destination> destinationOf(const QTreeWidgetItem* item) const;
    // Asks which section `page` is in and follows it, now (a timer does it otherwise).
    void followPage(quint32 page);

signals:
    // The reader clicked, or pressed Enter on, an item that has a destination.
    void destinationActivated(const vellora::Destination& destination);

protected:
    void paintEvent(QPaintEvent* event) override;
    void showEvent(QShowEvent* event) override;

private slots:
    void onOpened();
    void onOutline(quint64 request, const QList<vellora::OutlineItem>& items, bool more);
    void onOutlinePath(quint64 request, const QList<quint32>& path);
    void onRequestFailed(quint64 request);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void onItemExpanded(QTreeWidgetItem* item);
    void onItemActivated(QTreeWidgetItem* item);
    void onCurrentPageChanged(quint32 page);
    void followNow();

private:
    // One level of the tree: the children of one item, or the top.
    struct Level {
        QTreeWidgetItem* parent = nullptr; // the invisible root for the top level
        std::optional<quint32> after;
        quint32 already = 0;
        quint64 request = 0; // outstanding, or 0
        bool loaded = false;
        bool failed = false;
    };
    // The key of the top level in `m_levels`; the others are keyed by their parent's id.
    static constexpr qint64 kTop = -1;

    void requestLevel(qint64 key);
    std::optional<qint64> takeRequest(quint64 request);
    void addPlaceholder(QTreeWidgetItem* parent, const QString& text);
    void removePlaceholders(QTreeWidgetItem* parent);
    void advancePath();

    EngineSession* m_session;
    CanvasController* m_controller;
    QHash<quint32, QTreeWidgetItem*> m_items;
    QHash<quint32, Destination> m_destinations;
    QHash<qint64, Level> m_levels;
    QHash<quint64, qint64> m_requests; // request id -> level key
    // The section being followed: the page asked about, the request, and the path still to walk.
    quint32 m_followPage = 0;
    quint64 m_pathRequest = 0;
    QList<quint32> m_path;
    QTimer m_followTimer;
    // The selection is being moved by the view itself, which is not a jump.
    bool m_following = false;
};

} // namespace vellora
