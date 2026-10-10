// The Qt face of the engine client: owns one open document and turns the bridge's polled events
// into signals. The shell never parses PDF data; everything it knows about a document comes
// through here (ADR-0003, ADR-0004).
#pragma once

#include "bridge/Destination.h"
#include "rust/cxx.h"
#include "vellora-engine-client/src/bridge.rs.h"

#include <QByteArray>
#include <QImage>
#include <QList>
#include <QObject>
#include <QSizeF>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <optional>

namespace vellora {

class EngineSession : public QObject {
    Q_OBJECT

public:
    explicit EngineSession(QObject* parent = nullptr);
    ~EngineSession() override;

    // Sessions that exist now, open or not (tests use it to prove that closing a tab ends its
    // session).
    static int liveCount();

    // Starts an engine over `path`. Returns an empty string on success (`opened` follows), else
    // the error text; a session that failed to open stays closed.
    QString open(const QString& path);
    // Sends the password for a document that asked for one (`passwordRequested`). Returns an empty
    // string if it was sent (`opened` or `passwordRequested(true)` follows), else the error text.
    // The text is not logged or stored here, and the UTF-8 copy made for the call is wiped.
    QString submitPassword(const QString& password);
    void close();
    bool isOpen() const { return m_client.has_value(); }

    quint32 pageCount() const { return m_pageCount; }
    // Size in points; an invalid size until `opened`, and beyond the first 4096 pages.
    QSizeF pageSize(quint32 page) const;

    // Side of a tile in pixels, and the scale tiles are rendered at when `zoom` is asked for.
    static quint32 tilePixels();
    static float bucketScale(float zoom);
    // Size of the buffer `readTile` fills.
    quint32 slotBytes() const;

    // See the bridge: `Ready` tiles can be read at once, `Requested` ones arrive as `tileReady`.
    // A closed session, a down engine or an invalid tile answer `Full` (ask again later).
    TileTicket requestTile(quint32 page, float zoom, quint32 x, quint32 y, TilePriority priority);
    // Copies a ready tile (BGRx) into `out`, which is resized to `slotBytes()`.
    bool readTile(quint32 page, float zoom, quint32 x, quint32 y, QByteArray& out);
    void invalidatePage(quint32 page);
    bool cancel(quint64 request);

    // Page thumbnails: the whole page, `width` x `height` pixels (each 1 to `tilePixels()`) at the
    // bucketed `zoom`, from a cache of their own (64 MB by default) that tiles never evict from.
    // Same answers as `requestTile`; the engine renders them after every tile. The same page and
    // zoom must always be asked for with the same size.
    TileTicket requestThumbnail(quint32 page, float zoom, quint32 width, quint32 height);
    // Fills `out` (RGB32, `width` x `height`) with a ready thumbnail; false if it is not ready.
    bool readThumbnail(quint32 page, float zoom, quint32 width, quint32 height, QImage& out);

    // ---- navigation: the outline and the page labels ----
    // Each returns the id its answer carries, or 0 if it could not be sent (no document, engine
    // down, a value out of range). A failure of the engine arrives as `requestFailed`.
    //
    // One page of one level of the outline: the children of `parent` (the top level if empty),
    // after the item `after` (from the first if empty), at most `limit` (1 to 128). `already` is
    // how many items of the level the caller has.
    quint64 requestOutline(std::optional<quint32> parent, std::optional<quint32> after,
                           quint32 already, quint32 limit);
    // The outline items that lead to the section `page` is in.
    quint64 requestOutlinePath(quint32 page);
    // The labels of `count` pages (1 to 1024) from `first`.
    quint64 requestPageLabels(quint32 first, quint32 count);
    // The page that has the label `text`.
    quint64 findPageLabel(const QString& text);
    // Up to `limit` (1 to 256) links of `page` after the first `skip`.
    quint64 requestLinks(quint32 page, quint32 skip, quint32 limit);
    // Up to `limit` (1 to 8,192) characters of `page` after the first `skip`.
    quint64 requestTextPage(quint32 page, quint32 skip, quint32 limit);
    // Searches the whole text of the document, from the first page: `searchHits` (many, in page
    // order) and then `searchDone`. `text` is 1 to 1,024 bytes of UTF-8; with `regex` it is a
    // regular expression (one that does not parse is a `requestFailed` with the reason). A new
    // search ends the one before it; `cancel` stops one and silences it.
    quint64 requestSearch(const QString& text, bool caseSensitive, bool wholeWord, bool regex);

    // The operating-system id of the running engine process; 0 if none (for tests and diagnostics).
    quint32 engineProcessId() const { return m_client ? (*m_client)->engine_id() : 0; }

    // Drains the engine's events now (a timer does it every few milliseconds otherwise).
    void pollNow();

signals:
    // `repairs`: one line each for why the engine had to repair the document (empty if it did not).
    void opened(quint32 pageCount, const QStringList& repairs);
    // The document is encrypted and needs a password; `wrong` says the one just sent was refused.
    // The engine stays up and `submitPassword` may be called again. Replaces `requestFailed` for
    // these two answers.
    void passwordRequested(bool wrong);
    void tileReady(quint64 request);
    // Answers to the navigation requests above. `more`: the level goes on after the last item.
    void outlineReady(quint64 request, const QList<vellora::OutlineItem>& items, bool more);
    // Item ids from a top-level item down to the one that starts the section (empty if none does).
    void outlinePathReady(quint64 request, const QList<quint32>& path);
    // `defined`: the document has page labels; if not, `labels` are the page numbers.
    void pageLabelsReady(quint64 request, quint32 first, bool defined, const QStringList& labels);
    void pageFound(quint64 request, bool found, quint32 page);
    // `more`: ask again with `skip` advanced by the number received.
    void linksReady(quint64 request, quint32 page, const QList<vellora::Link>& links, bool more);
    // The characters of `page` from `skip`; `total` is how many the page has in all.
    void textReady(quint64 request, quint32 page, quint32 skip, quint32 total,
                   const QList<vellora::TextChar>& chars);
    // New hits of a search and how many pages it has searched (a message without hits shows
    // progress), then the end: how it ended, how many hits it reported in all, how many pages it
    // searched.
    void searchHits(quint64 request, const QList<vellora::SearchHit>& hits, quint32 pagesDone);
    void searchDone(quint64 request, vellora::SearchEnd end, quint32 hits, quint32 pagesDone);
    // `request` is 0 when the failure belongs to no request.
    void requestFailed(quint64 request, const QString& message);
    // Requests in `lost` will never be answered; ask again after `engineRestarted`.
    void engineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void engineRestarted();
    // The session cannot go on; every later request fails.
    void failed(const QString& reason);
    // The engine did not answer in time and the client killed it. After a Hello or Open timeout
    // the session is over (no `failed` follows); after a Tile timeout `engineCrashed` follows and
    // the engine restarts if it can.
    void engineTimedOut(vellora::TimeoutStage stage, const QString& message);
    // The file on disk is not what was opened; the restarting engine maps what is there now.
    // `replaced`: a different file is at the path, rather than the open file modified in place.
    void documentChanged(bool replaced);

private:
    void dispatch(const EngineEvent& event);

    std::optional<rust::Box<EngineClient>> m_client;
    QTimer m_timer;
    quint32 m_pageCount = 0;
};

} // namespace vellora
