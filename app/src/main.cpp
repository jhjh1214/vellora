#include "BuildInfo.h"
#include "MainWindow.h"
#include "SingleInstance.h"
#include "diagnostics/Application.h"
#include "diagnostics/DiagnosticsScript.h"
#include "diagnostics/UiWatchdog.h"
#include "settings/AppSettings.h"

#include <QCommandLineParser>
#include <QDir>
#include <QSettings>
#include <QTextStream>

int main(int argc, char** argv) {
    vellora::Application app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("Vellora"));
    QApplication::setOrganizationName(QStringLiteral("Vellora"));
    QApplication::setApplicationVersion(QLatin1String(vellora::buildinfo::kVersion));

    QCommandLineParser parser;
    parser.addHelpOption();
    parser.addPositionalArgument(QStringLiteral("file"), QStringLiteral("PDF file to open."),
                                 QStringLiteral("[file]"));
    // Hidden: runs a scripted scroll and zoom through the document, prints the frame-time report
    // and exits (the performance check and the packaging smoke test use it).
    QCommandLineOption scriptOption(QStringLiteral("diagnostics-script"),
                                    QStringLiteral("Run a scripted session, then exit."),
                                    QStringLiteral("name"));
    scriptOption.setFlags(QCommandLineOption::HiddenFromHelp);
    parser.addOption(scriptOption);
    parser.process(app);
    const QStringList files = parser.positionalArguments();
    const QString scriptName = parser.value(scriptOption);
    const bool scripted = parser.isSet(scriptOption);

    QTextStream err(stderr);
    if (scripted && !vellora::DiagnosticsScript::isScenario(scriptName)) {
        err << "vellora: unknown --diagnostics-script '" << scriptName
            << "' (known: scroll-zoom)\n";
        return 2;
    }
    if (scripted && files.isEmpty()) {
        err << "vellora: --diagnostics-script needs a file to open\n";
        return 2;
    }

    // Times the event handlers and `render()` against the frame budget (debug builds, or
    // VELLORA_UI_WATCHDOG=1; a scripted session always).
    vellora::UiWatchdog watchdog;
    app.setWatchdog(&watchdog);

    // Files are given relative to where the command was run; a running instance has another
    // working directory.
    const QStringList paths = vellora::resolvePaths(files, QDir::currentPath());

    // One instance per user: a second launch hands its files to the first and leaves. A scripted
    // session is a measurement and always runs on its own.
    vellora::SingleInstance instance;
    if (!scripted) {
        if (instance.forward(paths)) {
            return 0;
        }
        instance.listen();
    }

    // The settings of this user: native format (registry, plist, ini file). Nothing else in the
    // application touches them, and a scripted session does not either.
    QSettings nativeSettings;
    vellora::AppSettings settings(&nativeSettings);
    vellora::MainWindow window(scripted ? nullptr : &settings);
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
        err << "vellora: cannot open " << failed << '\n';
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
                             err << "vellora: engine failed: " << reason << '\n';
                             QApplication::exit(1);
                         });
        QObject::connect(&script, &vellora::DiagnosticsScript::finished, &app, [&] {
            watchdog.stop();
            QTextStream(stdout) << "diagnostics-script scroll-zoom: " << script.stepsDone()
                                << " pages, " << script.zoomsDone() << " zooms; "
                                << watchdog.summary() << '\n';
            QApplication::exit(0);
        });
    }
    return QApplication::exec();
}
