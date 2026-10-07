// UiWatchdog: a blocked event loop is noticed and measured.
#include "diagnostics/UiWatchdog.h"

#include <QSignalSpy>
#include <QTest>
#include <QThread>
#include <QTimer>

class TstUiWatchdog : public QObject {
    Q_OBJECT

private slots:
    void aBlockedEventLoopIsReportedWithItsLength() {
        // A generous threshold: CI machines hiccup, a deliberate 150 ms block is far above it.
        vellora::UiWatchdog watchdog(nullptr, 50);
        QSignalSpy stalled(&watchdog, &vellora::UiWatchdog::stalled);
        watchdog.start();
        QTest::qWait(50);
        QTimer::singleShot(0, [] { QThread::msleep(150); });
        QTest::qWait(400);
        watchdog.stop();

        QVERIFY2(watchdog.stallCount() >= 1, "the blocked loop was not noticed");
        QCOMPARE(stalled.size(), watchdog.stallCount());
        QVERIFY2(watchdog.longestStallMs() >= 100,
                 qPrintable(QString::number(watchdog.longestStallMs())));
        QVERIFY(watchdog.longestStallMs() < 1000);
        QCOMPARE(stalled.last().at(0).toLongLong(), watchdog.longestStallMs());
    }

    void anIdleEventLoopIsNotAStall() {
        vellora::UiWatchdog watchdog(nullptr, 100);
        watchdog.start();
        QTest::qWait(300);
        watchdog.stop();
        QCOMPARE(watchdog.stallCount(), 0);
        QCOMPARE(watchdog.longestStallMs(), 0);
    }

    void startingAgainClearsTheCounters() {
        vellora::UiWatchdog watchdog(nullptr, 20);
        watchdog.start();
        QTimer::singleShot(0, [] { QThread::msleep(80); });
        QTest::qWait(200);
        QVERIFY(watchdog.stallCount() >= 1);
        watchdog.start();
        QCOMPARE(watchdog.stallCount(), 0);
        QCOMPARE(watchdog.longestStallMs(), 0);
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

QTEST_MAIN(TstUiWatchdog)
#include "tst_ui_watchdog.moc"
