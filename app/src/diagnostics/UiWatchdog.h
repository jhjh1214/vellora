// Notices when the UI thread's event loop stops turning for too long (a stall: input and painting
// freeze). A timer ticks every few milliseconds on the UI thread itself; when a tick comes much
// later than it should have, the loop was blocked, and the gap is logged. It sees the stall after
// it ends and says how long it was, not what caused it; the log is what a developer scrolls through
// to check that nothing blocks the UI (M0 task 22 acceptance).
//
// On by default in builds without NDEBUG; `VELLORA_UI_WATCHDOG=1` or `=0` forces it either way.
#pragma once

#include <QElapsedTimer>
#include <QObject>
#include <QTimer>

namespace vellora {

class UiWatchdog : public QObject {
    Q_OBJECT

public:
    // How often the heartbeat runs, and how long the loop may be blocked before it is a stall.
    static constexpr int kTickMs = 4;
    static constexpr qint64 kStallMs = 16;

    explicit UiWatchdog(QObject* parent = nullptr, qint64 stallMs = kStallMs);

    // Debug builds, unless the environment says otherwise.
    static bool enabledByDefault();

    void start();
    void stop();

    // Stalls seen since `start`, and the longest one.
    int stallCount() const { return m_stalls; }
    qint64 longestStallMs() const { return m_longest; }

signals:
    // `ms` is how long the event loop was blocked.
    void stalled(qint64 ms);

private:
    void tick();

    QTimer m_timer;
    QElapsedTimer m_clock;
    qint64 m_stallMs;
    qint64 m_lastTickNs = 0;
    int m_stalls = 0;
    qint64 m_longest = 0;
};

} // namespace vellora
