// Local crash reports (ADR-0019). `install` starts the crash monitor (this same executable, run
// with
// `--crash-monitor`) and attaches a crash handler to the process; a crash then leaves a minidump in
// the crash folder, written by the monitor. Engine incidents (written by the engine client) go to
// the same folder. Nothing is ever uploaded: the next start only offers to show the folder and to
// open a prefilled GitHub issue, to which the user may attach files by hand.
#pragma once

#include <QDateTime>
#include <QList>
#include <QString>
#include <QUrl>

namespace vellora {

class CrashReports {
public:
    // The command-line flag that makes the executable run as the monitor.
    static constexpr auto kMonitorFlag = "--crash-monitor";

    struct Report {
        QString path;
        // true for a minidump of the application, false for a record of an engine incident.
        bool application = false;
        QDateTime modified;
    };

    // `crashes` in the log folder (`VELLORA_CRASH_DIR` overrides it).
    static QString defaultDirectory();
    // The folder in use while installed, else the default one.
    static QString directory();

    // Starts the monitor and the handler. `directory` empty means the default; `monitorExecutable`
    // empty means this executable. Returns an empty string on success, else why not (the
    // application then runs without crash dumps).
    static QString install(const QString& directory = {}, const QString& monitorExecutable = {});
    // Detaches the handler and ends the monitor.
    static void uninstall();
    static bool isInstalled();

    // Runs the monitor with this process's command line (`--crash-monitor <socket> <folder>`).
    static int runMonitor();

    // Crashes this process on purpose (an access violation), for `--crash-test` and tests.
    [[noreturn]] static void crashNow();

    // Reports in `directory` modified after `since` (a null time lists them all), newest first:
    // `crash-*.dmp` of the application and `engine-crash-*.txt` of the engine.
    static QList<Report> pending(const QString& directory, const QDateTime& since);

    // What goes beside a dump and into an issue: version, commit, build date, Qt and the system.
    static QString systemDescription();
    // The project's issue form with version and system filled in and the names (not the contents)
    // of the files listed. Plain text, percent-encoded, short enough for any browser.
    static QUrl issueUrl(const QList<Report>& reports);
};

} // namespace vellora