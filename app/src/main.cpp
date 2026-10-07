#include "MainWindow.h"
#include "diagnostics/UiWatchdog.h"

#include <QApplication>

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("Vellora"));
    QApplication::setOrganizationName(QStringLiteral("Vellora"));

    // Logs every stall of the event loop above 16 ms (debug builds; VELLORA_UI_WATCHDOG=1 forces
    // it).
    vellora::UiWatchdog watchdog;
    if (vellora::UiWatchdog::enabledByDefault()) {
        watchdog.start();
    }

    vellora::MainWindow window;
    window.show();

    // `vellora <file.pdf>` opens the file at start-up.
    const QStringList arguments = QApplication::arguments();
    if (arguments.size() > 1) {
        window.openDocument(arguments.at(1));
    }
    return QApplication::exec();
}
