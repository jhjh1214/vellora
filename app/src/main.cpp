#include "MainWindow.h"
#include "diagnostics/Application.h"
#include "diagnostics/DiagnosticsScript.h"
#include "diagnostics/UiWatchdog.h"

#include <QCommandLineParser>
#include <QTextStream>

int main(int argc, char** argv) {
    vellora::Application app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("Vellora"));
    QApplication::setOrganizationName(QStringLiteral("Vellora"));

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

    vellora::MainWindow window;
    watchdog.watch(window.canvas().canvas());
    if (scripted || vellora::UiWatchdog::enabledByDefault()) {
        watchdog.start();
    }
    window.show();

    // `vellora <file.pdf>` opens the file at start-up.
    if (!files.isEmpty()) {
        window.openDocument(files.first());
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
