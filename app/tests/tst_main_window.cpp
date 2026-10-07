// The main window opens a document through the engine and shows its page count.
#include "MainWindow.h"

#include <QSignalSpy>
#include <QTest>

class TstMainWindow : public QObject {
    Q_OBJECT

private slots:
    void showsThePageCountOfAnOpenedFile() {
        vellora::MainWindow window;
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);

        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(window.windowTitle().contains(QStringLiteral("golden.pdf")));
        QVERIFY(opened.wait(60'000));
        QVERIFY2(window.documentStatus().contains(QStringLiteral("3 page")),
                 qPrintable(window.documentStatus()));
    }

    void reportsAFileThatCannotBeOpened() {
        vellora::MainWindow window;
        QVERIFY(!window.openDocument(QStringLiteral("no/such/file.pdf")));
        QVERIFY2(window.documentStatus().startsWith(QStringLiteral("Cannot open")),
                 qPrintable(window.documentStatus()));
    }
};

QTEST_MAIN(TstMainWindow)
#include "tst_main_window.moc"
