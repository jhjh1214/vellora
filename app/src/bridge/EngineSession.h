// The Qt face of the engine client: owns one open document and turns the bridge's polled events
// into signals. The shell never parses PDF data; everything it knows about a document comes
// through here (ADR-0003, ADR-0004).
#pragma once

#include "rust/cxx.h"
#include "vellora-engine-client/src/bridge.rs.h"

#include <QByteArray>
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
