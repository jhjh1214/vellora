// The main window opens a document through the engine and shows its page count, and says when the
// file had to be repaired.
#include "MainWindow.h"
#include "RepairBar.h"
#include "SyntheticPdf.h"

#include <QDialog>
#include <QFile>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

namespace {

// A valid file whose `startxref` keyword is spoilt, so that the engine rebuilds the table by
// scanning; PDFium and `cos` both manage, and the engine reports the repair.
QString writeDamagedPdf(const QTemporaryDir& dir) {
    QByteArray pdf = syntheticPdf(2);
    const qsizetype at = pdf.lastIndexOf("startxref");
    pdf.replace(at, 9, "startxraf");
    const QString path = dir.filePath(QStringLiteral("damaged.pdf"));
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write(pdf) != pdf.size()) {
        return {};
    }
    return path;
}

} // namespace

class TstMainWindow : public QObject {
    Q_OBJECT

private slots:
    void showsThePageCountOfAnOpenedFile() {
        vellora::MainWindow window;
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);

        QVERIFY2(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)),
                 qPrintable(window.documentStatus()));
        QVERIFY(window.windowTitle().contains(QStringLiteral("golden.pdf")));
        QVERIFY(opened.wait(60'000));
        QVERIFY2(window.documentStatus().contains(QStringLiteral("3 page")),
                 qPrintable(window.documentStatus()));
    }

    void saysNothingAboutACleanFile() {
        vellora::MainWindow window;
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY2(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)),
                 qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));
        QVERIFY(!window.repairBar().isVisible());
        QVERIFY(window.repairBar().reasons().isEmpty());
        QVERIFY(!window.documentStatus().contains(QStringLiteral("repaired")));
    }

    void showsABarWithDetailsForARepairedFile() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = writeDamagedPdf(dir);
        QVERIFY(!path.isEmpty());

        vellora::MainWindow window;
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY2(window.openDocument(path), qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));

        vellora::RepairBar& bar = window.repairBar();
        QVERIFY(bar.isVisible());
        QVERIFY(bar.text().contains(QStringLiteral("damaged and has been repaired")));
        QVERIFY(!bar.reasons().isEmpty());
        QCOMPARE(opened.first().at(1).toStringList(), bar.reasons());
        QVERIFY(window.documentStatus().contains(QStringLiteral("repaired")));

        // Details lists every reason, as plain text.
        QDialog* details = bar.showDetails();
        QVERIFY(details != nullptr);
        QVERIFY(details->isVisible());
        const auto* list = details->findChild<QPlainTextEdit*>();
        QVERIFY(list != nullptr);
        QCOMPARE(list->toPlainText(), bar.reasons().join(QLatin1Char('\n')));
        details->close();

        // Dismiss hides it for this document.
        bar.dismissButton()->click();
        QVERIFY(!bar.isVisible());

        // Another document starts with a clean slate: a clean file shows no bar...
        opened.clear();
        QVERIFY2(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)),
                 qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));
        QVERIFY(!bar.isVisible());
        QVERIFY(bar.reasons().isEmpty());
        QVERIFY(bar.showDetails() == nullptr);

        // ...and a damaged one shows it again, even though it was dismissed before.
        opened.clear();
        QVERIFY2(window.openDocument(path), qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));
        QVERIFY(bar.isVisible());
    }

    void showsEngineTextAsPlainText() {
        vellora::MainWindow window;
        for (const auto* label : window.findChildren<QLabel*>()) {
            QCOMPARE(label->textFormat(), Qt::PlainText);
        }
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
