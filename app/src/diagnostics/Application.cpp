#include "diagnostics/Application.h"

#include "diagnostics/UiWatchdog.h"

#include <QAbstractEventDispatcher>
#include <QElapsedTimer>
#include <QThread>

namespace vellora {

namespace {

// Delivering these runs the window's paint and its present, which blocks until the display's next
// refresh. That wait is not ours; the drawing we do inside it is measured by `render()`.
bool containsPresent(QEvent::Type type) {
    return type == QEvent::Paint || type == QEvent::UpdateRequest || type == QEvent::UpdateLater;
}

} // namespace

bool Application::notify(QObject* receiver, QEvent* event) {
    // The event loop's own pump (on Windows, a timer event for the dispatcher that delivers the
    // posted events) is not a handler: whatever it runs is delivered through notify() again, and
    // each of those is timed on its own. Timing the pump would charge the whole batch to one
    // handler.
    if (qobject_cast<QAbstractEventDispatcher*>(receiver) != nullptr) {
        return QApplication::notify(receiver, event);
    }
    const bool timed = m_watchdog != nullptr && m_watchdog->running() && m_depth == 0 &&
                       !containsPresent(event->type()) && receiver->thread() == thread();
    ++m_depth;
    if (!timed) {
        const bool handled = QApplication::notify(receiver, event);
        --m_depth;
        return handled;
    }
    // The event may delete its receiver, so remember what the log line needs first (the meta
    // object is static).
    const QEvent::Type type = event->type();
    const QMetaObject* meta = receiver->metaObject();
    QElapsedTimer clock;
    clock.start();
    const bool handled = QApplication::notify(receiver, event);
    const double ms = static_cast<double>(clock.nsecsElapsed()) / 1e6;
    --m_depth;
    m_watchdog->recordHandler(ms, meta, type);
    return handled;
}

} // namespace vellora
