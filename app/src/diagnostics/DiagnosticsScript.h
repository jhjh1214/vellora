// A scripted session for measuring the canvas: scrolls through the document one page per step and
// zooms in and out on the way, the way a user flinging through a long file would. Used by Qt tests
// and by the hidden `--diagnostics-script` flag of the app (and later by the packaging smoke
// test).
//
// The steps run from a timer inside the event loop, like input events, never by sleeping: a sleep
// would itself block the loop. How far the script gets in a given time depends on how fast frames
// present, so the work is counted in steps, not seconds.
#pragma once

#include "canvas/CanvasController.h"

#include <QObject>
#include <QTimer>

namespace vellora {

class DiagnosticsScript : public QObject {
    Q_OBJECT

public:
    struct Options {
        // Pages to scroll through, one per step (fewer if the document is shorter).
        int pages = 2000;
        // Zoom in/out pairs spread evenly over the steps: 20 zoom operations in all.
        int zoomCycles = 10;
        int stepIntervalMs = 8;
        double zoomFactor = 1.25;
    };

    explicit DiagnosticsScript(CanvasController* controller, QObject* parent = nullptr);
    DiagnosticsScript(CanvasController* controller, Options options, QObject* parent = nullptr);

    // The named scenarios of `--diagnostics-script`; empty if `name` is not one.
    static bool isScenario(const QString& name);

    // Starts the script from the top of the document; `finished` follows the last step.
    void start();
    bool running() const { return m_timer.isActive(); }

    int stepsDone() const { return m_step; }
    int zoomsDone() const { return m_zooms; }

signals:
    void finished();

private:
    void step();

    CanvasController* m_controller;
    Options m_options;
    QTimer m_timer;
    int m_steps = 0;
    int m_step = 0;
    int m_zooms = 0;
};

} // namespace vellora
