#include "diagnostics/UiWatchdog.h"

#include <QLoggingCategory>
#include <algorithm>

Q_LOGGING_CATEGORY(lcWatchdog, "vellora.watchdog")

namespace vellora {

UiWatchdog::UiWatchdog(QObject* parent, qint64 stallMs) : QObject(parent), m_stallMs(stallMs) {
    // Millisecond timers: the default coarse timer on Windows ticks every 15 ms, which would
    // itself look like a stall.
    m_timer.setTimerType(Qt::PreciseTimer);
    m_timer.setInterval(kTickMs);
    connect(&m_timer, &QTimer::timeout, this, &UiWatchdog::tick);
}

bool UiWatchdog::enabledByDefault() {
    const QByteArray setting = qgetenv("VELLORA_UI_WATCHDOG");
    if (setting == "1") {
        return true;
    }
    if (setting == "0") {
        return false;
    }
#ifdef NDEBUG
    return false;
#else
    return true;
#endif
}

void UiWatchdog::start() {
    m_stalls = 0;
    m_longest = 0;
    m_clock.start();
    m_lastTickNs = m_clock.nsecsElapsed();
    m_timer.start();
}

void UiWatchdog::stop() {
    m_timer.stop();
}

void UiWatchdog::tick() {
    const qint64 now = m_clock.nsecsElapsed();
    // The loop was blocked for the part of the gap beyond the tick interval.
    const qint64 blockedMs = (now - m_lastTickNs) / 1'000'000 - kTickMs;
    m_lastTickNs = now;
    if (blockedMs <= m_stallMs) {
        return;
    }
    ++m_stalls;
    m_longest = std::max(m_longest, blockedMs);
    qCWarning(lcWatchdog, "UI event loop stalled for %lld ms", static_cast<long long>(blockedMs));
    emit stalled(blockedMs);
}

} // namespace vellora
