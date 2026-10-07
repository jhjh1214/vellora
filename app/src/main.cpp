#include "MainWindow.h"

#include <QApplication>

int main(int argc, char** argv) {
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("Vellora"));
    QApplication::setOrganizationName(QStringLiteral("Vellora"));

    vellora::MainWindow window;
    window.show();

    // `vellora <file.pdf>` opens the file at start-up.
    const QStringList arguments = QApplication::arguments();
    if (arguments.size() > 1) {
        window.openDocument(arguments.at(1));
    }
    return QApplication::exec();
}
