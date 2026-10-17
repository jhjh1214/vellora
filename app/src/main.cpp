#include "BuildInfo.h"
#include "I18n.h"
#include "MainWindow.h"
#include "SingleInstance.h"
#include "commands/ShortcutsPage.h"
#include "diagnostics/Application.h"
#include "diagnostics/CrashReports.h"
#include "diagnostics/DiagnosticsScript.h"
#include "diagnostics/Logging.h"
#include "diagnostics/UiWatchdog.h"
#include "settings/AppSettings.h"

#include <QCommandLineParser>
#include <QDir>
#include <QFile>
#include <QSettings>
#include <QTextStream>
#include <QTimer>
#include <cstring>

int main(int argc, char** argv) {
    // The crash monitor is this same executable started by a running instance (ADR-0019): no
    // windows, no Qt application, it only writes the dump if the instance crashes.
    if (argc > 1 && std::strcmp(argv[1], vellora::CrashReports::kMonitorFlag) == 0) {
        return vellora::CrashReports::runMonitor();
    }

    vellora::Application app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("Vellora"));
    QApplication::setOrganizationName(QStringLiteral("Vellora"));
    QApplication::setApplicationVersion(QLatin1String(vellora::buildinfo::kVersion));
    // Before any window or message: VELLORA_LOCALE=pseudo|pseudo-rtl|<language> (src/I18n.h).
    const QString translationNote = vellora::i18n::installFromEnvironment(app);

    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addPositionalArgument(QStringLiteral("file"),
                                 QCoreApplication::translate("main", "PDF file to open."),
                                 QStringLiteral("[file]"));
    // Hidden: runs a scripted scroll and zoom through the document, prints the frame-time report
    // and exits (the performance check and the packaging smoke test use it).
    // i18n-ok: hidden options are never shown, so there is nothing to translate
    QCommandLineOption scriptOption(QStringLiteral("diagnostics-script"),
                                    QStringLiteral("Run a scripted session, then exit."),
                                    QStringLiteral("name"));
    scriptOption.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(scriptOption);
    // Hidden: installs crash reporting and then crashes on purpose (the crash-report tests).
    // i18n-ok: hidden option
    QCommandLineOption crashOption(QStringLiteral("crash-test"),
                                   QStringLiteral("Crash on purpose, to test crash reports."));
    crashOption.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(crashOption);
    // Hidden: writes the keyboard shortcuts page of the user manual from the command registry and
    // exits (CI regenerates docs/user/shortcuts.md with it and fails if the page differs).
    // i18n-ok: hidden option
    QCommandLineOption shortcutsOption(QStringLiteral("export-shortcuts"),
                                       QStringLiteral("Write the shortcuts page, then exit."),
                                       QStringLiteral("file"));
    shortcutsOption.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(shortcutsOption);
    parser.process(app);
    const QStringList files = parser.positionalArguments();
    const QString scriptName = parser.value(scriptOption);
    const bool scripted = parser.isSet(scriptOption);
    const bool crashTest = parser.isSet(crashOption);
    // A measurement or a test run stands alone: no other instance, none of the user's settings.
    const bool isolated = scripted || crashTest;

    QTextStream err(stderr);
    if (parser.isSet(shortcutsOption)) {
        // No settings, no log, no crash monitor, no other instance: the registry only.
        vellora::MainWindow window(nullptr);
        QFile out(parser.value(shortcutsOption));
        const QByteArray page = vellora::shortcutsMarkdown(window.commands()).toUtf8();
        if (!out.open(QIODevice::WriteOnly | QIODevice::Truncate) ||
            out.write(page) != page.size()) {
            // i18n-ok: console diagnostics, not the UI
            err << "vellora: cannot write " << out.fileName() << ": " << out.errorString() << '\n';
            return 1;
        }
        return 0;
    }
    if (scripted && !vellora::DiagnosticsScript::isScenario(scriptName)) {
        // i18n-ok: console diagnostics, not the UI
        err << "vellora: unknown --diagnostics-script '" << scriptName
            << "' (known: scroll-zoom)\n";
        return 2;
    }
    if (scripted && files.isEmpty()) {
        // i18n-ok: console diagnostics, not the UI
        err << "vellora: --diagnostics-script needs a file to open\n";
        return 2;
    }

    // Times the event handlers and `render()` against the frame budget (debug builds, or
    // VELLORA_UI_WATCHDOG=1; a scripted session always).
    vellora::UiWatchdog watchdog;
    app.setWatchdog(&watchdog);

    // The application log: rotating files in the platform's log folder. A measurement run
    // (`--diagnostics-script`) does not write one.
    if (!scripted) {
        const QString problem = vellora::Logging::start();
        if (!problem.isEmpty()) {
            // i18n-ok: console diagnostics, not the UI
            err << "vellora: no log file: " << problem << '\n';
        } else {
            qInfo("Vellora %s (%s) started", vellora::buildinfo::kVersion,
                  vellora::buildinfo::kCommit);
            qInfo("language: %s", qPrintable(translationNote));
        }
    }

    // Crash dumps of this process, written by a monitor process (ADR-0019). Not for a measurement
    // run, which must not start extra processes.
    if (!scripted) {
        const QString problem = vellora::CrashReports::install();
        if (!problem.isEmpty()) {
            qWarning("crash dumps are not available: %s", qPrintable(problem));
        }
    }

    // Files are given relative to where the command was run; a running instance has another
    // working directory.
    const QStringList paths = vellora::resolvePaths(files, QDir::currentPath());

    // One instance per user: a second launch hands its files to the first and leaves. A scripted
    // session is a measurement and always runs on its own.
    vellora::SingleInstance instance;
    if (!isolated) {
        if (instance.forward(paths)) {
            return 0;
        }
        instance.listen();
    }

    // The settings of this user: native format (registry, plist, ini file). Nothing else in the
    // application touches them, and a scripted session does not either.
    QSettings nativeSettings;
    vellora::AppSettings settings(&nativeSettings);
    vellora::MainWindow window(isolated ? nullptr : &settings);
    QObject::connect(
        &window, &vellora::MainWindow::tabAdded, &window,
        [&watchdog](vellora::DocumentTab* tab) { watchdog.watch(tab->canvas().canvas()); });
    watchdog.watch(window.canvas().canvas());
    if (scripted || vellora::UiWatchdog::enabledByDefault()) {
        watchdog.start();
    }
    window.show();
    QObject::connect(&instance, &vellora::SingleInstance::filesReceived, &window,
                     [&window](const QStringList& received) {
                         window.openDocuments(received);
                         window.setWindowState(window.windowState() & ~Qt::WindowMinimized);
                         window.raise();
                         window.activateWindow();
                     });

    // `vellora a.pdf b.pdf` opens each file in a tab.
    for (const QString& failed : window.openDocuments(paths)) {
        // i18n-ok: console diagnostics, not the UI
        err << "vellora: cannot open " << failed << '\n';
    }

    if (crashTest) {
        QTimer::singleShot(200, &app, [] { vellora::CrashReports::crashNow(); });
    } else if (!scripted) {
        // Crash reports written since the user last looked: offer to show them.
        window.checkForCrashReports();
    }

    vellora::DiagnosticsScript script(window.canvas().controller());
    if (scripted) {
        bool started = false;
        QObject::connect(&window.session(), &vellora::EngineSession::opened, &script, [&] {
            if (!started) { // an engine restart opens the document again
                started = true;
                script.start();
            }
        });
        QObject::connect(&window.session(), &vellora::EngineSession::failed, &app,
                         [&err](const QString& reason) {
                             // i18n-ok: console diagnostics, not the UI
                             err << "vellora: engine failed: " << reason << '\n';
                             QApplication::exit(1);
                         });
        QObject::connect(&script, &vellora::DiagnosticsScript::finished, &app, [&] {
            watchdog.stop();
            // i18n-ok: the report that tests and CI read
            QTextStream(stdout) << "diagnostics-script scroll-zoom: " << script.stepsDone()
                                << " pages, " << script.zoomsDone() << " zooms; "
                                << watchdog.summary() << '\n';
            QApplication::exit(0);
        });
    }
    const int result = QApplication::exec();
    vellora::CrashReports::uninstall();
    vellora::Logging::stop();
    return result;
}
