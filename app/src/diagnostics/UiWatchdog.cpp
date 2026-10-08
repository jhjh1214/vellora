#include "diagnostics/UiWatchdog.h"

#include "canvas/CanvasWidget.h"

#include <QLoggingCategory>
#include <algorithm>

Q_LOGGING_CATEGORY(lcWatchdog, "vellora.watchdog")

namespace vellora {

UiWatchdog::UiWatchdog(QObject* parent, double budgetMs) : QObject(parent), m_budgetMs(budgetMs) {}

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
    m_handlers = {};
    m_frames = {};
    m_running = true;
}

void UiWatchdog::stop() {
    m_running = false;
}

void UiWatchdog::watch(CanvasWidget* canvas) {
    connect(canvas, &CanvasWidget::frameRendered, this, &UiWatchdog::recordFrame);
}

void UiWatchdog::recordHandler(double ms, const QMetaObject* receiver, QEvent::Type type) {
    if (!m_running) {
        return;
    }
    ++m_handlers.samples;
    if (ms <= m_budgetMs) {
        m_handlers.worstMs = std::max(m_handlers.worstMs, ms);
        return;
    }
    // Only the slow path builds a description of the event.
    record(m_handlers, Kind::Handler, ms,
           QStringLiteral("event %1 for %2")
               .arg(static_cast<int>(type))
               .arg(receiver != nullptr ? QString::fromLatin1(receiver->className())
                                        : QStringLiteral("?")));
}

void UiWatchdog::recordFrame(double ms) {
    if (!m_running) {
        return;
    }
    ++m_frames.samples;
    if (ms <= m_budgetMs) {
        m_frames.worstMs = std::max(m_frames.worstMs, ms);
        return;
    }
    record(m_frames, Kind::Frame, ms, QStringLiteral("render()"));
}

void UiWatchdog::record(Counters& counters, Kind kind, double ms, const QString& what) {
    ++counters.overBudget;
    counters.worstMs = std::max(counters.worstMs, ms);
    qCWarning(lcWatchdog, "%s took %.2f ms (budget %.1f ms)", qPrintable(what), ms, m_budgetMs);
    emit overBudget(kind, ms);
}

QString UiWatchdog::summary() const {
    const auto line = [this](const char* name, const Counters& c) {
        return QStringLiteral("%1: %2 samples, %3 over %4 ms, worst %5 ms")
            .arg(QLatin1String(name))
            .arg(c.samples)
            .arg(c.overBudget)
            .arg(m_budgetMs, 0, 'f', 1)
            .arg(c.worstMs, 0, 'f', 2);
    };
    return line("handlers", m_handlers) + QStringLiteral("; ") + line("frames", m_frames);
}

} // namespace vellora
