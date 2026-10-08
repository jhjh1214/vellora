// UiWatchdog and Application: handler and frame times are measured, and what goes over the
// budget is counted.
#include "VelloraTestMain.h"
#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"
#include "canvas/CanvasWidget.h"
#include "diagnostics/UiWatchdog.h"

#include <QSignalSpy>
#include <QThread>
#include <QTimer>

namespace {

constexpr auto kSlow = static_cast<QEvent::Type>(QEvent::User + 1);

// Takes 30 ms to handle a `kSlow` or an update request, and counts those it was given.
class Sleeper : public QObject {
public:
    int slowEvents = 0;

    bool event(QEvent* event) override {
        if (event->type() == kSlow || event->type() == QEvent::UpdateRequest) {
            ++slowEvents;
            QThread::msleep(30);
            return true;
        }
        return QObject::event(event);
    }
};

vellora::Application* application() {
    return qobject_cast<vellora::Application*>(QCoreApplication::instance());
}

} // namespace

class TstUiWatchdog : public QObject {
    Q_OBJECT

private slots:
    void cleanup() { application()->setWatchdog(nullptr); }

    void samplesOverTheBudgetAreCountedAndTheWorstIsKept() {
        vellora::UiWatchdog watchdog; // the 8 ms default
        QCOMPARE(vellora::UiWatchdog::kBudgetMs, 8.0);
        watchdog.start();
        watchdog.recordHandler(1.0, &QObject::staticMetaObject, QEvent::Timer);
        watchdog.recordHandler(8.0, &QObject::staticMetaObject, QEvent::Timer); // at the budget
        watchdog.recordHandler(9.5, &QObject::staticMetaObject, QEvent::Timer);
        watchdog.recordFrame(7.9);
        watchdog.recordFrame(8.1);
        watchdog.recordFrame(20.0);

        QCOMPARE(watchdog.handlers().samples, quint64(3));
        QCOMPARE(watchdog.handlers().overBudget, quint64(1));
        QCOMPARE(watchdog.handlers().worstMs, 9.5);
        QCOMPARE(watchdog.frames().samples, quint64(3));
        QCOMPARE(watchdog.frames().overBudget, quint64(2));
        QCOMPARE(watchdog.frames().worstMs, 20.0);
        QCOMPARE(watchdog.overBudgetTotal(), quint64(3));
        QVERIFY(watchdog.summary().contains(QStringLiteral("frames: 3 samples, 2 over 8.0 ms")));
    }

    void overBudgetIsAnnouncedWithItsKindAndLength() {
        vellora::UiWatchdog watchdog;
        QSignalSpy over(&watchdog, &vellora::UiWatchdog::overBudget);
        watchdog.start();
        watchdog.recordFrame(3.0);
        QCOMPARE(over.size(), 0);
        watchdog.recordFrame(12.5);
        watchdog.recordHandler(9.0, &QObject::staticMetaObject, QEvent::Timer);
        QCOMPARE(over.size(), 2);
        QCOMPARE(over.at(0).at(0).value<vellora::UiWatchdog::Kind>(),
                 vellora::UiWatchdog::Kind::Frame);
        QCOMPARE(over.at(0).at(1).toDouble(), 12.5);
        QCOMPARE(over.at(1).at(0).value<vellora::UiWatchdog::Kind>(),
                 vellora::UiWatchdog::Kind::Handler);
    }

    void nothingIsRecordedWhileStoppedAndStartingClearsTheCounters() {
        vellora::UiWatchdog watchdog;
        watchdog.recordFrame(50.0); // not started
        QCOMPARE(watchdog.frames().samples, quint64(0));
        watchdog.start();
        watchdog.recordFrame(50.0);
        QCOMPARE(watchdog.frames().overBudget, quint64(1));
        watchdog.stop();
        watchdog.recordFrame(50.0);
        QCOMPARE(watchdog.frames().samples, quint64(1)); // readable after stop, not growing
        watchdog.start();
        QCOMPARE(watchdog.frames().samples, quint64(0));
        QCOMPARE(watchdog.frames().worstMs, 0.0);
    }

    void aCanvasFrameIsRecordedThroughItsSignal() {
        vellora::EngineSession session;
        vellora::CanvasController controller(&session);
        vellora::CanvasWidget canvas(&session, &controller);
        vellora::UiWatchdog watchdog;
        watchdog.watch(&canvas);
        watchdog.start();
        emit canvas.frameRendered(2.0);
        emit canvas.frameRendered(11.0);
        QCOMPARE(watchdog.frames().samples, quint64(2));
        QCOMPARE(watchdog.frames().overBudget, quint64(1));
        QCOMPARE(watchdog.frames().worstMs, 11.0);
    }

    void aSlowHandlerIsMeasuredByTheApplication() {
        vellora::UiWatchdog watchdog;
        application()->setWatchdog(&watchdog);
        watchdog.start();
        QTimer::singleShot(0, [] { QThread::msleep(40); });
        QTest::qWait(200);
        watchdog.stop();

        QVERIFY2(watchdog.handlers().overBudget >= 1, "the slow handler was not noticed");
        QVERIFY2(watchdog.handlers().worstMs >= 30.0,
                 qPrintable(QString::number(watchdog.handlers().worstMs)));
        QVERIFY(watchdog.handlers().worstMs < 1000.0);
        QCOMPARE(watchdog.frames().samples, quint64(0));
    }

    void anIdleEventLoopIsNotCounted() {
        // A generous budget: a CI machine can be descheduled for longer than 8 ms.
        vellora::UiWatchdog watchdog(nullptr, 100.0);
        application()->setWatchdog(&watchdog);
        watchdog.start();
        QTest::qWait(300);
        watchdog.stop();
        QCOMPARE(watchdog.handlers().overBudget, quint64(0));
    }

    void aNestedDeliveryIsNotCountedTwice() {
        Sleeper sleeper;
        vellora::UiWatchdog watchdog;
        application()->setWatchdog(&watchdog);
        watchdog.start();
        // One timer handler that sends a 30 ms event: two nested `notify` calls, one slow delivery.
        QTimer::singleShot(0, [&sleeper] {
            QEvent event(kSlow);
            QCoreApplication::sendEvent(&sleeper, &event);
        });
        QTest::qWait(200);
        watchdog.stop();
        QCOMPARE(watchdog.handlers().overBudget, quint64(1));
    }

    void thePresentIsNotAHandler() {
        // Paint and update requests contain the window's blocking present, which is not ours.
        Sleeper sleeper;
        vellora::UiWatchdog watchdog;
        application()->setWatchdog(&watchdog);
        watchdog.start();
        // Posted, so the event loop delivers it from outside any handler, like the real one.
        QCoreApplication::postEvent(&sleeper, new QEvent(QEvent::UpdateRequest));
        QTRY_COMPARE(sleeper.slowEvents, 1);
        QCOMPARE(watchdog.handlers().overBudget, quint64(0));
        // Control: the same 30 ms in any other event is counted.
        QCoreApplication::postEvent(&sleeper, new QEvent(kSlow));
        QTRY_COMPARE(sleeper.slowEvents, 2);
        QCOMPARE(watchdog.handlers().overBudget, quint64(1));
        watchdog.stop();
    }

    void theEnvironmentOverridesTheBuildDefault() {
        qputenv("VELLORA_UI_WATCHDOG", "1");
        QVERIFY(vellora::UiWatchdog::enabledByDefault());
        qputenv("VELLORA_UI_WATCHDOG", "0");
        QVERIFY(!vellora::UiWatchdog::enabledByDefault());
        qunsetenv("VELLORA_UI_WATCHDOG");
#ifdef NDEBUG
        QVERIFY(!vellora::UiWatchdog::enabledByDefault());
#else
        QVERIFY(vellora::UiWatchdog::enabledByDefault());
#endif
    }
};

VELLORA_TEST_MAIN(TstUiWatchdog)
#include "tst_ui_watchdog.moc"
