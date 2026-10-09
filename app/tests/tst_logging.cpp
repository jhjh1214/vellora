// The application log: Qt messages and the engine's log lines end up in rotating files, and the
// "Open Log Folder" command shows the folder.
#include "MainWindow.h"
#include "diagnostics/Logging.h"

#include <QDir>
#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

namespace {

QString logText(const QString& directory) {
    vellora::Logging::flush();
    QString text;
    const QStringList names = QDir(directory).entryList({QStringLiteral("vellora*.log")});
    for (const QString& name : names) {
        QFile file(QDir(directory).filePath(name));
        if (file.open(QIODevice::ReadOnly)) {
            text += QString::fromUtf8(file.readAll());
        }
    }
    return text;
}

} // namespace

class TstLogging : public QObject {
    Q_OBJECT

private slots:
    void cleanup() { vellora::Logging::stop(); }

    void qtMessagesGoToTheFileAtTheirLevel() {
        QTemporaryDir dir;
        const QString folder = dir.filePath(QStringLiteral("a/b/logs"));
        QVERIFY(!vellora::Logging::isStarted());
        QCOMPARE(vellora::Logging::start(folder), QString());
        QVERIFY(vellora::Logging::isStarted());
        QCOMPARE(vellora::Logging::directory(), folder);

        // QTest prints these too (the earlier handler stays in the chain); expected, not failures.
        QTest::ignoreMessage(QtWarningMsg, "marker-warning-31d");
        QTest::ignoreMessage(QtCriticalMsg, "marker-critical-77a");
        qWarning("marker-warning-31d");
        qCritical("marker-critical-77a");
        qInfo("marker-info-5be");
        qDebug("marker-debug-02c");

        const QString text = logText(folder);
        QVERIFY2(text.contains(QStringLiteral("WARN qt: marker-warning-31d")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("ERROR qt: marker-critical-77a")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("INFO qt: marker-info-5be")), qPrintable(text));
        // Debug is below the default level.
        QVERIFY2(!text.contains(QStringLiteral("marker-debug-02c")), qPrintable(text));

        // A second start is refused; stopping restores the earlier handler and ends the file.
        QVERIFY(!vellora::Logging::start(folder).isEmpty());
        vellora::Logging::stop();
        QVERIFY(!vellora::Logging::isStarted());
        qInfo("marker-after-stop-9e4");
        QVERIFY(!logText(folder).contains(QStringLiteral("marker-after-stop-9e4")));
        // And it can be started again.
        QCOMPARE(vellora::Logging::start(folder), QString());
        qInfo("marker-restart-2a1");
        QVERIFY(logText(folder).contains(QStringLiteral("marker-restart-2a1")));
        // The first run's lines are still there: the file is appended to.
        QVERIFY(logText(folder).contains(QStringLiteral("marker-info-5be")));
    }

    void aFolderThatCannotBeUsedIsReported() {
        QTemporaryDir dir;
        QFile blocker(dir.filePath(QStringLiteral("file")));
        QVERIFY(blocker.open(QIODevice::WriteOnly));
        blocker.close();
        // A folder below a regular file cannot be created.
        QVERIFY(!vellora::Logging::start(dir.filePath(QStringLiteral("file/logs"))).isEmpty());
        QVERIFY(!vellora::Logging::isStarted());
    }

    void theEnginesLinesAndTheShellsLinesLandInTheSameFile() {
        QTemporaryDir dir;
        const QString folder = dir.filePath(QStringLiteral("logs"));
        QCOMPARE(vellora::Logging::start(folder), QString());
        {
            vellora::MainWindow window;
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
            QVERIFY(opened.wait(60'000));
            // Closing the tab ends the engine, which says so on its way out.
        }
        QString text;
        QTRY_VERIFY_WITH_TIMEOUT((text = logText(folder)).contains(QStringLiteral("session ended")),
                                 20'000);
        QVERIFY2(text.contains(QStringLiteral("engine started")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("document opened")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("engine: ")), qPrintable(text));
        // The file name of the document is not at info level.
        QVERIFY2(!text.contains(QStringLiteral("golden.pdf")), qPrintable(text));
    }

    void openLogFolderShowsTheFolder() {
        QTemporaryDir dir;
        const QString folder = dir.filePath(QStringLiteral("not-yet-there"));
        QCOMPARE(vellora::Logging::start(folder), QString());
        vellora::MainWindow window;
        QString shown;
        window.setFolderOpener([&](const QString& path) {
            shown = path;
            return true;
        });
        QVERIFY(window.commands().find(QStringLiteral("help.openLogFolder")) != nullptr);
        QCOMPARE(window.commands().find(QStringLiteral("help.openLogFolder"))->title,
                 QStringLiteral("Open Log Folder"));
        QVERIFY(window.commands().run(QStringLiteral("help.openLogFolder")));
        QCOMPARE(shown, folder);
        QVERIFY(QDir(folder).exists());

        // Not started: it shows the default folder, and makes sure it exists.
        vellora::Logging::stop();
        QVERIFY(window.openLogFolder());
        QCOMPARE(shown, vellora::Logging::defaultDirectory());
    }

    void theDefaultFolderIsPerUserAndNamedAfterTheApplication() {
        const QString folder = vellora::Logging::defaultDirectory();
        QVERIFY(QDir::isAbsolutePath(folder));
        QVERIFY2(folder.contains(QStringLiteral("ellora"), Qt::CaseInsensitive),
                 qPrintable(folder));
    }
};

QTEST_MAIN(TstLogging)
#include "tst_logging.moc"