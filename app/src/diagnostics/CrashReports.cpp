#include "diagnostics/CrashReports.h"

#include "BuildInfo.h"
#include "bridge/EngineSession.h"

#include <QCoreApplication>
#include <QDir>
#include <QFileInfo>
#include <QMutex>
#include <QMutexLocker>
#include <QSysInfo>
#include <QUrlQuery>
#include <algorithm>
#include <exception>
#include <optional>

namespace vellora {

namespace {

QMutex g_mutex;
std::optional<rust::Box<CrashHandle>> g_crash;
QString g_directory;

QString toQString(const rust::String& text) {
    return QString::fromUtf8(text.data(), static_cast<qsizetype>(text.size()));
}

rust::Str toStr(const QByteArray& utf8) {
    return rust::Str(utf8.constData(), static_cast<size_t>(utf8.size()));
}

// What the issue form takes for "Operating system" (its dropdown options).
QString issueSystem() {
#if defined(Q_OS_WIN)
    return QStringLiteral("Windows");
#elif defined(Q_OS_MACOS)
    return QStringLiteral("macOS");
#elif defined(Q_OS_LINUX)
    return QStringLiteral("Linux");
#else
    return QStringLiteral("Other");
#endif
}

} // namespace

QString CrashReports::defaultDirectory() {
    return toQString(vellora::default_crash_directory());
}

QString CrashReports::directory() {
    QMutexLocker lock(&g_mutex);
    return g_crash ? g_directory : defaultDirectory();
}

QString CrashReports::systemDescription() {
    return QStringLiteral("version: %1\ncommit: %2\nbuilt: %3\nqt: %4\nsystem: %5 (%6)")
        .arg(QLatin1String(buildinfo::kVersion), QLatin1String(buildinfo::kCommit),
             QLatin1String(buildinfo::kBuildDate), QString::fromLatin1(qVersion()),
             QSysInfo::prettyProductName(), QSysInfo::currentCpuArchitecture());
}

QString CrashReports::install(const QString& directory, const QString& monitorExecutable) {
    QMutexLocker lock(&g_mutex);
    if (g_crash) {
        return QObject::tr("Crash reporting is already installed.");
    }
    const QString folder = directory.isEmpty() ? defaultDirectory() : directory;
    const QString monitor =
        monitorExecutable.isEmpty() ? QCoreApplication::applicationFilePath() : monitorExecutable;
    try {
        g_crash.emplace(vellora::install_crash_handler(
            toStr(folder.toUtf8()), toStr(monitor.toUtf8()), toStr(systemDescription().toUtf8())));
    } catch (const std::exception& error) {
        return QString::fromUtf8(error.what());
    }
    g_directory = folder;
    return {};
}

void CrashReports::uninstall() {
    QMutexLocker lock(&g_mutex);
    if (g_crash) {
        (*g_crash)->stop();
        g_crash.reset();
    }
}

bool CrashReports::isInstalled() {
    QMutexLocker lock(&g_mutex);
    return g_crash.has_value();
}

int CrashReports::runMonitor() {
    return vellora::run_crash_monitor();
}

void CrashReports::crashNow() {
    // A write through a null pointer: an access violation on Windows, SIGSEGV elsewhere, like the
    // crashes this is meant to stand in for.
    volatile int* nothing = nullptr;
    *nothing = 1;
    // Not reached; the handler ends the process. Keeps the function [[noreturn]] if it were.
    std::abort();
}

QList<CrashReports::Report> CrashReports::pending(const QString& directory,
                                                  const QDateTime& since) {
    QList<Report> reports;
    const QDir dir(directory);
    const QFileInfoList files =
        dir.entryInfoList({QStringLiteral("crash-*.dmp"), QStringLiteral("engine-crash-*.txt")},
                          QDir::Files | QDir::NoSymLinks, QDir::Time);
    for (const QFileInfo& file : files) {
        const QDateTime modified = file.lastModified();
        if (since.isValid() && modified <= since) {
            continue;
        }
        Report report;
        report.path = file.absoluteFilePath();
        report.application = file.suffix() == QLatin1String("dmp");
        report.modified = modified;
        reports.append(report);
    }
    std::sort(reports.begin(), reports.end(),
              [](const Report& a, const Report& b) { return a.modified > b.modified; });
    return reports;
}

QUrl CrashReports::issueUrl(const QList<Report>& reports) {
    // Only file names: the user decides whether to attach anything.
    QStringList names;
    for (const Report& report : reports) {
        names.append(QFileInfo(report.path).fileName());
    }
    constexpr qsizetype kMaxListed = 10;
    if (names.size() > kMaxListed) {
        names = names.mid(0, kMaxListed);
        names.append(QStringLiteral("..."));
    }
    const QString what =
        QStringLiteral(
            "Vellora recorded a crash report (written locally; nothing was uploaded).\n\n"
            "Reports in the crash folder:\n%1\n\n"
            "I have not attached any file. A .dmp file holds the stacks of the threads, not the "
            "heap; "
            "look at it before attaching it, issues are public.\n\n"
            "What I was doing:\n")
            .arg(names.isEmpty() ? QStringLiteral("(none)")
                                 : QStringLiteral("- ") + names.join(QStringLiteral("\n- ")));
    QUrlQuery query;
    query.addQueryItem(QStringLiteral("template"), QStringLiteral("bug_report.yml"));
    query.addQueryItem(QStringLiteral("title"),
                       QStringLiteral("Crash: Vellora %1 on %2")
                           .arg(QLatin1String(buildinfo::kVersion), issueSystem()));
    query.addQueryItem(QStringLiteral("what-happened"), what);
    query.addQueryItem(QStringLiteral("version"), QStringLiteral("%1 (commit %2)")
                                                      .arg(QLatin1String(buildinfo::kVersion),
                                                           QLatin1String(buildinfo::kCommit)));
    query.addQueryItem(QStringLiteral("os"), issueSystem());
    QUrl url(QLatin1String(buildinfo::kRepository) + QStringLiteral("/issues/new"));
    url.setQuery(query);
    return url;
}

} // namespace vellora