// The application object, which times how long delivering each event takes (measurement (a) of
// UiWatchdog). `notify` is the one place every event passes through on its way to a receiver:
// handlers, timers and queued slots included. The event dispatcher's own pump is not timed; what it
// delivers is, event by event.
#pragma once

#include <QApplication>

namespace vellora {

class UiWatchdog;

class Application : public QApplication {
    Q_OBJECT

public:
    using QApplication::QApplication;

    // The watchdog that receives the timings (not owned); null stops timing.
    void setWatchdog(UiWatchdog* watchdog) { m_watchdog = watchdog; }

    bool notify(QObject* receiver, QEvent* event) override;

private:
    UiWatchdog* m_watchdog = nullptr;
    // `notify` is re-entered by `sendEvent` from inside a handler. Only the outermost delivery is
    // timed, otherwise the same milliseconds would be counted once per level.
    int m_depth = 0;
};

} // namespace vellora
