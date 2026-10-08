// Measures the part of every frame that our code controls, and counts what goes over budget.
// Two measurements, because the UI thread's event loop also spends time in things we do not
// control (the vsynced present of the window, which blocks for a whole refresh interval):
//
//  (a) handlers: time spent delivering one event to its receiver, so our event handlers, timers
//      and queued slots. `Application::notify` feeds it. Paint and update requests are left out:
//      they contain the blocking present, and the drawing we do in them is measured by (b).
//  (b) frames: time spent in `CanvasWidget::render()` per frame, which excludes the present.
//
// A sample over `kBudgetMs` (our 8 ms share of a 60 Hz frame) is "over budget": it is counted,
// the slowest is kept, and a line is logged. Tests read the counters; `--diagnostics-script` of
// the app prints them.
//
// On by default in builds without NDEBUG; `VELLORA_UI_WATCHDOG=1` or `=0` forces it either way.
#pragma once

#include <QEvent>
#include <QObject>

namespace vellora {

class CanvasWidget;

class UiWatchdog : public QObject {
    Q_OBJECT

public:
    // Our share of a 16.7 ms frame.
    static constexpr double kBudgetMs = 8.0;

    enum class Kind { Handler, Frame };
    Q_ENUM(Kind)

    struct Counters {
        quint64 samples = 0;
        quint64 overBudget = 0;
        double worstMs = 0.0;
    };

    explicit UiWatchdog(QObject* parent = nullptr, double budgetMs = kBudgetMs);

    // Debug builds, unless the environment says otherwise.
    static bool enabledByDefault();

    // Clears the counters and starts recording; `stop` ends it (the counters stay readable).
    void start();
    void stop();
    bool running() const { return m_running; }

    // Records the frames of `canvas` (its `frameRendered` signal).
    void watch(CanvasWidget* canvas);

    // `ms` spent delivering an event of `type` to an object of class `receiver`.
    void recordHandler(double ms, const QMetaObject* receiver, QEvent::Type type);
    // `ms` spent in one `render()`.
    void recordFrame(double ms);

    Counters handlers() const { return m_handlers; }
    Counters frames() const { return m_frames; }
    quint64 overBudgetTotal() const { return m_handlers.overBudget + m_frames.overBudget; }

    // "handlers: 1234 samples, 0 over 8 ms, worst 1.2 ms; frames: ..." for logs and reports.
    QString summary() const;

signals:
    void overBudget(vellora::UiWatchdog::Kind kind, double ms);

private:
    void record(Counters& counters, Kind kind, double ms, const QString& what);

    double m_budgetMs;
    bool m_running = false;
    Counters m_handlers;
    Counters m_frames;
};

} // namespace vellora
