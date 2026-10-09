#include "diagnostics/DiagnosticsScript.h"

#include <algorithm>

namespace vellora {

namespace {

constexpr auto kScrollAndZoom = "scroll-zoom";

} // namespace

DiagnosticsScript::DiagnosticsScript(CanvasController* controller, QObject* parent)
    : DiagnosticsScript(controller, Options{}, parent) {}

DiagnosticsScript::DiagnosticsScript(CanvasController* controller, Options options, QObject* parent)
    : QObject(parent), m_controller(controller), m_options(options) {
    m_timer.setTimerType(Qt::PreciseTimer);
    m_timer.setInterval(m_options.stepIntervalMs);
    connect(&m_timer, &QTimer::timeout, this, &DiagnosticsScript::step);
}

bool DiagnosticsScript::isScenario(const QString& name) {
    return name == QLatin1String(kScrollAndZoom);
}

void DiagnosticsScript::start() {
    const int pageCount = static_cast<int>(m_controller->pageCount());
    m_steps = std::clamp(m_options.pages, 0, std::max(pageCount - 1, 0));
    m_step = 0;
    m_zooms = 0;
    m_controller->setScrollPosition(QPointF(0.0, 0.0));
    m_timer.start();
}

void DiagnosticsScript::step() {
    if (m_step >= m_steps) {
        m_timer.stop();
        emit finished();
        return;
    }
    ++m_step;
    // Zoom operations alternate in and out, and are due evenly across the steps; all of them are
    // done by the last step, however few steps there are.
    const int zoomOps = 2 * m_options.zoomCycles;
    const QPointF centre(m_controller->viewportSize().width() / 2.0,
                         m_controller->viewportSize().height() / 2.0);
    while (m_zooms < zoomOps &&
           static_cast<long long>(m_zooms) * m_steps <= static_cast<long long>(m_step) * zoomOps) {
        const bool in = m_zooms % 2 == 0;
        m_controller->zoomBy(in ? m_options.zoomFactor : 1.0 / m_options.zoomFactor, centre);
        ++m_zooms;
    }
    // The top of page `m_step`, at whatever zoom the script is at now; in a view that shows one
    // row at a time, that page turns up.
    m_controller->goToPage(static_cast<quint32>(m_step));
}

} // namespace vellora
