// Local crash reports: a real crash of the application leaves a minidump written by the monitor
// process, the next start offers to show it, and nothing is uploaded.
#include "BuildInfo.h"
#include "CrashDialog.h"
#include "MainWindow.h"
#include "diagnostics/CrashReports.h"
#include "settings/AppSettings.h"

#include <QDir>
#include <QFile>
#include <QLabel>
#include <QProcess>
#include <QPushButton>
#include <QSettings>
#include <QTemporaryDir>
#include <QTest>
#include <QUrlQuery>

namespace {

QString touch(const QString& path, const QByteArray& content, const QDateTime& when) {
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write(content) != content.size()) {
        return {};
    }
    file.close();
    // Set after closing: closing must not move the time.
    if (!file.open(QIODevice::ReadWrite) ||
        !file.setFileTime(when, QFileDevice::FileModificationTime)) {
        return {};
    }
    return path;
}

} // namespace

class TstCrash : public QObject {
    Q_OBJECT

private slots:
    void cleanup() { vellora::CrashReports::uninstall(); }

    void aRealCrashLeavesADumpAndTheMonitorLeaves() {
        QTemporaryDir dir;
        const QString crashes = dir.filePath(QStringLiteral("crashes"));
        QProcessEnvironment environment = QProcessEnvironment::systemEnvironment();
        environment.insert(QStringLiteral("VELLORA_CRASH_DIR"), crashes);
        environment.insert(QStringLiteral("VELLORA_LOG_DIR"), dir.filePath(QStringLiteral("logs")));
        environment.insert(QStringLiteral("QT_QPA_PLATFORM"), QStringLiteral("offscreen"));

        QProcess app;
        app.setProcessEnvironment(environment);
        app.start(QStringLiteral(VELLORA_APP_PATH), {QStringLiteral("--crash-test")});
        QVERIFY2(app.waitForStarted(30'000), qPrintable(app.errorString()));
        QVERIFY2(app.waitForFinished(60'000), "the application did not crash within a minute");
        // It was killed by the crash, not ended by itself.
        QVERIFY2(app.exitStatus() == QProcess::CrashExit || app.exitCode() != 0,
                 qPrintable(QString::number(app.exitCode())));

        // The monitor writes the dump while the application waits; both files are there.
        QStringList dumps;
        QTRY_VERIFY_WITH_TIMEOUT(
            !(dumps = QDir(crashes).entryList({QStringLiteral("crash-*.dmp")})).isEmpty(), 20'000);
        QCOMPARE(dumps.size(), 1);
        QFile dump(QDir(crashes).filePath(dumps.first()));
        QVERIFY(dump.open(QIODevice::ReadOnly));
        const QByteArray content = dump.readAll();
        QVERIFY2(content.size() > 4096, qPrintable(QString::number(content.size())));
        QCOMPARE(content.left(4), QByteArray("MDMP"));
        // The text file beside it says which build crashed.
        const QString sidecarName = dumps.first().chopped(4) + QStringLiteral(".txt");
        QFile sidecar(QDir(crashes).filePath(sidecarName));
        QVERIFY(sidecar.open(QIODevice::ReadOnly));
        const QString text = QString::fromUtf8(sidecar.readAll());
        QVERIFY2(text.contains(QLatin1String(vellora::buildinfo::kVersion)), qPrintable(text));
        QVERIFY2(text.contains(QLatin1String(vellora::buildinfo::kCommit)), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("heap")), qPrintable(text));
        // The monitor removed its socket and is gone.
        QTRY_VERIFY_WITH_TIMEOUT(QDir(crashes).entryList({QStringLiteral("*.sock")}).isEmpty(),
                                 20'000);

        // The next start finds it.
        const auto found = vellora::CrashReports::pending(crashes, QDateTime());
        QCOMPARE(found.size(), 1);
        QVERIFY(found.first().application);
    }

    void aCleanRunLeavesNoReport() {
        QTemporaryDir dir;
        const QString crashes = dir.filePath(QStringLiteral("crashes"));
        QVERIFY(!vellora::CrashReports::isInstalled());
        QCOMPARE(vellora::CrashReports::install(crashes, QStringLiteral(VELLORA_APP_PATH)),
                 QString());
        QVERIFY(vellora::CrashReports::isInstalled());
        QCOMPARE(vellora::CrashReports::directory(), crashes);
        QVERIFY(
            !vellora::CrashReports::install(crashes, QStringLiteral(VELLORA_APP_PATH)).isEmpty());
        vellora::CrashReports::uninstall();
        QVERIFY(!vellora::CrashReports::isInstalled());
        QVERIFY(vellora::CrashReports::pending(crashes, QDateTime()).isEmpty());
        QTRY_VERIFY_WITH_TIMEOUT(QDir(crashes).entryList(QDir::Files).isEmpty(), 10'000);
    }

    void aMonitorThatCannotStartIsReported() {
        QTemporaryDir dir;
        const QString problem =
            vellora::CrashReports::install(dir.filePath(QStringLiteral("crashes")),
                                           dir.filePath(QStringLiteral("no-such-program")));
        QVERIFY(!problem.isEmpty());
        QVERIFY(!vellora::CrashReports::isInstalled());
    }

    void pendingListsOnlyNewReportsNewestFirst() {
        QTemporaryDir dir;
        const QString folder = dir.path();
        const QDateTime base = QDateTime::currentDateTime().addDays(-1);
        touch(QDir(folder).filePath(QStringLiteral("crash-100.dmp")), "MDMP", base.addSecs(10));
        touch(QDir(folder).filePath(QStringLiteral("crash-100.txt")), "info", base.addSecs(10));
        touch(QDir(folder).filePath(QStringLiteral("engine-crash-200.txt")), "engine",
              base.addSecs(20));
        touch(QDir(folder).filePath(QStringLiteral("notes.txt")), "not a report", base.addSecs(30));
        touch(QDir(folder).filePath(QStringLiteral("vellora.log")), "log", base.addSecs(40));

        const auto all = vellora::CrashReports::pending(folder, QDateTime());
        QCOMPARE(all.size(), 2);         // the sidecar, the notes and the log are not reports
        QVERIFY(!all.at(0).application); // newest first
        QVERIFY(all.at(1).application);
        QVERIFY(all.at(0).path.endsWith(QStringLiteral("engine-crash-200.txt")));

        QCOMPARE(vellora::CrashReports::pending(folder, base.addSecs(10)).size(), 1);
        QCOMPARE(vellora::CrashReports::pending(folder, base.addSecs(20)).size(), 0);
        QVERIFY(vellora::CrashReports::pending(dir.filePath(QStringLiteral("nope")), QDateTime())
                    .isEmpty());
    }

    void theIssueLinkCarriesNamesAndNoContents() {
        QTemporaryDir dir;
        const QString secret = QStringLiteral("SECRET-STACK-CONTENT-7c1");
        const QString dump = touch(QDir(dir.path()).filePath(QStringLiteral("crash-5.dmp")),
                                   secret.toUtf8(), QDateTime::currentDateTime());
        const auto reports = vellora::CrashReports::pending(dir.path(), QDateTime());
        const QUrl url = vellora::CrashReports::issueUrl(reports);
        QVERIFY(url.toString().startsWith(QLatin1String(vellora::buildinfo::kRepository)));
        QCOMPARE(url.path(), QUrl(QLatin1String(vellora::buildinfo::kRepository)).path() +
                                 QStringLiteral("/issues/new"));
        const QUrlQuery query(url);
        QCOMPARE(query.queryItemValue(QStringLiteral("template")),
                 QStringLiteral("bug_report.yml"));
        const QString version =
            QUrl::fromPercentEncoding(query.queryItemValue(QStringLiteral("version")).toUtf8());
        QVERIFY2(version.contains(QLatin1String(vellora::buildinfo::kVersion)),
                 qPrintable(version));
        QVERIFY2(version.contains(QLatin1String(vellora::buildinfo::kCommit)), qPrintable(version));
        const QString os = query.queryItemValue(QStringLiteral("os"));
        QVERIFY2(QStringList({"Windows", "Linux", "macOS", "Other"}).contains(os), qPrintable(os));
        const QString body = QUrl::fromPercentEncoding(
            query.queryItemValue(QStringLiteral("what-happened"), QUrl::FullyDecoded).toUtf8());
        QVERIFY2(body.contains(QStringLiteral("crash-5.dmp")), qPrintable(body));
        QVERIFY2(body.contains(QStringLiteral("nothing was uploaded")), qPrintable(body));
        // Only the name of the file is in the link, never what is in it.
        QVERIFY(!url.toString().contains(secret));
        QVERIFY(!dump.isEmpty());
        QVERIFY(url.toEncoded().size() < 4000);

        // Many reports do not make an unusable link.
        QList<vellora::CrashReports::Report> many;
        for (int i = 0; i < 200; ++i) {
            many.append({QStringLiteral("/very/long/folder/name/crash-%1.dmp").arg(i), true, {}});
        }
        QVERIFY(vellora::CrashReports::issueUrl(many).toEncoded().size() < 4000);
    }

    void theDialogOffersThreeChoicesAndSaysWhatItHolds() {
        const QList<vellora::CrashReports::Report> reports = {
            {QStringLiteral("/x/crash-1.dmp"), true, QDateTime::currentDateTime()},
            {QStringLiteral("/x/engine-crash-2.txt"), false, QDateTime::currentDateTime()}};
        vellora::CrashDialog dialog(reports);
        const QString text = dialog.message()->text();
        QCOMPARE(dialog.message()->textFormat(), Qt::PlainText);
        QVERIFY2(text.contains(QStringLiteral("closed unexpectedly")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("Nothing was sent anywhere")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("no file is attached")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("check it before you share it")), qPrintable(text));
        QCOMPARE(dialog.openFolderButton()->text(), QStringLiteral("Open crash folder"));
        QCOMPARE(dialog.reportButton()->text(), QStringLiteral("Report on GitHub"));
        QCOMPARE(dialog.dismissButton()->text(), QStringLiteral("Dismiss"));

        // Closing it any other way is Dismiss, and it says so once.
        int chosen = 0;
        QObject::connect(&dialog, &vellora::CrashDialog::chosen, &dialog, [&] { ++chosen; });
        dialog.show();
        dialog.close();
        QCOMPARE(dialog.choice(), vellora::CrashDialog::Choice::Dismiss);
        QCOMPARE(chosen, 1);
        dialog.reject();
        QCOMPARE(chosen, 1);

        // An engine-only report is worded for the engine.
        vellora::CrashDialog engineOnly({reports.last()});
        QVERIFY2(engineOnly.message()->text().contains(QStringLiteral("page renderer stopped")),
                 qPrintable(engineOnly.message()->text()));
    }

    void theNextStartAsksOnceAndNothingIsUploaded() {
        QTemporaryDir dir;
        const QString crashes = dir.filePath(QStringLiteral("crashes"));
        QVERIFY(QDir().mkpath(crashes));
        const QDateTime base = QDateTime::currentDateTime().addSecs(-3600);
        touch(QDir(crashes).filePath(QStringLiteral("crash-1.dmp")), "MDMP....", base);

        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        vellora::AppSettings settings(&backing);
        vellora::MainWindow window(&settings);
        window.show();
        QString shownFolder;
        QUrl openedUrl;
        window.setFolderOpener([&](const QString& folder) {
            shownFolder = folder;
            return true;
        });
        window.setUrlOpener([&](const QUrl& url) {
            openedUrl = url;
            return true;
        });

        // Nothing there: no dialog.
        QVERIFY(window.checkForCrashReports(dir.filePath(QStringLiteral("empty"))) == nullptr);

        QDialog* shown = window.checkForCrashReports(crashes);
        QVERIFY(shown != nullptr);
        QVERIFY(shown->isVisible());
        auto* dialog = qobject_cast<vellora::CrashDialog*>(shown);
        QVERIFY(dialog != nullptr);
        dialog->reportButton()->click();
        QVERIFY(openedUrl.isValid());
        QVERIFY(openedUrl.toString().contains(QStringLiteral("crash-1.dmp")));
        QVERIFY(shownFolder.isEmpty()); // the report button does not open the folder
        // Asked once: the same report is not offered again, not even by a new window over the
        // same settings.
        QVERIFY(window.checkForCrashReports(crashes) == nullptr);
        vellora::MainWindow again(&settings);
        QVERIFY(again.checkForCrashReports(crashes) == nullptr);

        // A report that arrives later is offered, and "Open crash folder" shows the folder.
        touch(QDir(crashes).filePath(QStringLiteral("engine-crash-9.txt")), "later",
              QDateTime::currentDateTime().addSecs(60));
        shown = window.checkForCrashReports(crashes);
        QVERIFY(shown != nullptr);
        qobject_cast<vellora::CrashDialog*>(shown)->openFolderButton()->click();
        QCOMPARE(shownFolder, crashes);
        QVERIFY(window.checkForCrashReports(crashes) == nullptr);

        // Dismiss opens nothing.
        shownFolder.clear();
        openedUrl.clear();
        touch(QDir(crashes).filePath(QStringLiteral("crash-3.dmp")), "MDMP....",
              QDateTime::currentDateTime().addSecs(120));
        shown = window.checkForCrashReports(crashes);
        QVERIFY(shown != nullptr);
        qobject_cast<vellora::CrashDialog*>(shown)->dismissButton()->click();
        QVERIFY(shownFolder.isEmpty() && !openedUrl.isValid());
        QVERIFY(window.checkForCrashReports(crashes) == nullptr);
    }

    void withoutSettingsTheWindowStillAsksOnlyOnce() {
        QTemporaryDir dir;
        touch(dir.filePath(QStringLiteral("crash-1.dmp")), "MDMP....",
              QDateTime::currentDateTime().addSecs(-60));
        vellora::MainWindow window;
        QDialog* shown = window.checkForCrashReports(dir.path());
        QVERIFY(shown != nullptr);
        qobject_cast<vellora::CrashDialog*>(shown)->dismissButton()->click();
        QVERIFY(window.checkForCrashReports(dir.path()) == nullptr);
    }
};

QTEST_MAIN(TstCrash)
#include "tst_crash.moc"