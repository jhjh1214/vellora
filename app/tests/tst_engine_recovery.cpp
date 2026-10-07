// The engine is killed from outside while a document is open: the window shows a non-modal notice,
// the client restarts the engine, the notice goes away and tiles come back. Real engine process.
#include "KillProcess.h"
#include "MainWindow.h"

#include <QSignalSpy>
#include <QTest>

namespace {

constexpr int kWaitMs = 60'000;

// Whether the tile at the first position of the current frame can be read (it has arrived).
bool firstVisibleTileReady(vellora::MainWindow& window) {
    const vellora::Frame frame = window.canvas().controller()->frame();
    if (frame.tiles.isEmpty()) {
        return false;
    }
    const vellora::TileDraw& tile = frame.tiles.first();
    QByteArray pixels;
    return window.session().readTile(tile.page, tile.scale, tile.x, tile.y, pixels);
}

} // namespace

class TstEngineRecovery : public QObject {
    Q_OBJECT

private slots:
    void theWindowShowsANoticeWhileTheEngineRestartsAndTilesComeBack() {
        vellora::MainWindow window;
        window.show();
        vellora::EngineSession& session = window.session();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QSignalSpy crashed(&session, &vellora::EngineSession::engineCrashed);
        QSignalSpy restarted(&session, &vellora::EngineSession::engineRestarted);

        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        QTRY_VERIFY_WITH_TIMEOUT(firstVisibleTileReady(window), kWaitMs);
        QVERIFY(!window.canvas().bannerVisible());

        // The engine dies under the window.
        const quint32 pid = session.engineProcessId();
        QVERIFY(pid != 0);
        killProcess(pid);
        QVERIFY(crashed.wait(kWaitMs));
        QCOMPARE(crashed.last().at(1).toBool(), true); // will restart
        // Non-modal notice, shown at once; the window is not blocked (this loop is running).
        QVERIFY(window.canvas().bannerVisible());
        QVERIFY2(window.canvas().bannerText().contains(QStringLiteral("retrying")),
                 qPrintable(window.canvas().bannerText()));

        // A new engine takes over the same document; the notice goes away.
        // `EngineRestarted` may already have arrived with the crash (the signals are separate
        // events, but one poll can deliver both), so count rather than wait.
        QTRY_VERIFY_WITH_TIMEOUT(restarted.size() >= 1, kWaitMs);
        QTRY_COMPARE_WITH_TIMEOUT(opened.size(), 2, kWaitMs);
        QVERIFY(!window.canvas().bannerVisible());
        QVERIFY(session.engineProcessId() != 0);
        QVERIFY(session.engineProcessId() != pid);

        // And it renders: a view the old engine never drew (another zoom bucket) fills with tiles.
        window.canvas().controller()->setZoom(2.0, QPointF(100.0, 100.0));
        QTRY_VERIFY_WITH_TIMEOUT(firstVisibleTileReady(window), kWaitMs);
        QCOMPARE(window.canvas().controller()->frame().tiles.first().scale, 2.0F);
    }

    void aWindowThatCannotRestartTheEngineSaysSo() {
        vellora::MainWindow window;
        window.show();
        vellora::EngineSession& session = window.session();
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QSignalSpy crashed(&session, &vellora::EngineSession::engineCrashed);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));

        // Crashing again and again, with no tile delivered in between, exhausts the restarts.
        bool willRestart = true;
        int kills = 0;
        while (willRestart && kills < 10) {
            QTRY_VERIFY_WITH_TIMEOUT(session.engineProcessId() != 0, kWaitMs);
            const int seen = crashed.size();
            killProcess(session.engineProcessId());
            QTRY_VERIFY_WITH_TIMEOUT(crashed.size() > seen, kWaitMs);
            willRestart = crashed.last().at(1).toBool();
            ++kills;
        }
        QVERIFY2(!willRestart, "the client kept restarting the engine");
        QVERIFY(window.canvas().bannerVisible());
        QVERIFY2(window.canvas().bannerText().contains(QStringLiteral("could not be restarted")),
                 qPrintable(window.canvas().bannerText()));
    }
};

QTEST_MAIN(TstEngineRecovery)
#include "tst_engine_recovery.moc"
