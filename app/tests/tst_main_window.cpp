// The main window opens a document through the engine and shows its page count, and says when the
// file had to be repaired.
#include "MainWindow.h"
#include "PasswordDialog.h"
#include "RepairBar.h"
#include "SyntheticPdf.h"

#include <QDialog>
#include <QFile>
#include <QLabel>
#include <QLineEdit>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

namespace {

// Everything Qt logs (at any level) while a test runs, to prove that a password is not in it.
QStringList g_logged;
QtMessageHandler g_previousHandler = nullptr;

void recordMessage(QtMsgType type, const QMessageLogContext& context, const QString& message) {
    g_logged.append(message);
    if (g_previousHandler) {
        g_previousHandler(type, context, message);
    }
}

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

    void asksForThePasswordOfAnEncryptedFileAndOpensIt() {
        g_logged.clear();
        g_previousHandler = qInstallMessageHandler(recordMessage);
        vellora::MainWindow window;
        window.show();
        struct Ask {
            QString fileName;
            int attempt;
            int maxAttempts;
            bool wrong;
        };
        QList<Ask> asked;
        const QStringList answers = {QStringLiteral("hunter2-WRONG"), QStringLiteral("user-pw")};
        window.setPasswordProvider([&](const QString& fileName, int attempt, int maxAttempts,
                                       bool wrong) -> std::optional<QString> {
            asked.append({fileName, attempt, maxAttempts, wrong});
            return answers.value(asked.size() - 1);
        });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);

        QVERIFY2(window.openDocument(QStringLiteral(VELLORA_PROTECTED_PDF)),
                 qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));
        qInstallMessageHandler(g_previousHandler);

        QCOMPARE(asked.size(), 2);
        QCOMPARE(asked.at(0).fileName, QStringLiteral("r6-aes-256-user-password.pdf"));
        QCOMPARE(asked.at(0).attempt, 1);
        QCOMPARE(asked.at(0).maxAttempts, 3);
        QVERIFY(!asked.at(0).wrong);
        QCOMPARE(asked.at(1).attempt, 2);
        QVERIFY(asked.at(1).wrong);
        QVERIFY2(window.documentStatus().contains(QStringLiteral("1 page")),
                 qPrintable(window.documentStatus()));
        QVERIFY(window.windowTitle().contains(QStringLiteral("r6-aes-256")));
        // Neither password reached a log line.
        for (const QString& line : std::as_const(g_logged)) {
            QVERIFY2(!line.contains(QStringLiteral("hunter2")) &&
                         !line.contains(QStringLiteral("user-pw")),
                     qPrintable(line));
        }
    }

    void givesUpAfterThreeWrongPasswords() {
        vellora::MainWindow window;
        window.show();
        int asks = 0;
        window.setPasswordProvider([&](const QString&, int, int, bool) -> std::optional<QString> {
            ++asks;
            return QStringLiteral("wrong");
        });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_PROTECTED_PDF)));
        QTRY_VERIFY_WITH_TIMEOUT(!window.session().isOpen(), 60'000);
        QCOMPARE(asks, vellora::MainWindow::kMaxPasswordAttempts);
        QCOMPARE(opened.size(), 0);
        QVERIFY2(window.documentStatus().startsWith(QStringLiteral("Cannot open")),
                 qPrintable(window.documentStatus()));
        QCOMPARE(window.session().pageCount(), 0U);
        QCOMPARE(window.windowTitle(), QStringLiteral("Vellora"));
    }

    void cancellingThePromptClosesTheDocument() {
        vellora::MainWindow window;
        window.show();
        int asks = 0;
        window.setPasswordProvider([&](const QString&, int, int, bool) -> std::optional<QString> {
            ++asks;
            return std::nullopt;
        });
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_PROTECTED_PDF)));
        QTRY_VERIFY_WITH_TIMEOUT(!window.session().isOpen(), 60'000);
        QCOMPARE(asks, 1);
        QVERIFY2(window.documentStatus().contains(QStringLiteral("password is required")),
                 qPrintable(window.documentStatus()));
    }

    void neverAsksForADocumentThatNeedsNoPassword() {
        vellora::MainWindow window;
        window.show();
        int asks = 0;
        window.setPasswordProvider([&](const QString&, int, int, bool) -> std::optional<QString> {
            ++asks;
            return QStringLiteral("unused");
        });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(60'000));
        QCOMPARE(asks, 0);
    }

    void aPromptForAnOldDocumentDoesNotAskAboutTheNewOne() {
        vellora::MainWindow window;
        window.show();
        int asks = 0;
        window.setPasswordProvider([&](const QString&, int, int, bool) -> std::optional<QString> {
            ++asks;
            return QStringLiteral("user-pw");
        });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        // The window queues its question when the engine asks. The user opens another file from
        // the very next slot of the same signal, i.e. before the question can be shown.
        bool reopened = false;
        connect(&window.session(), &vellora::EngineSession::passwordRequested, &window, [&] {
            if (!reopened) {
                reopened = true;
                QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
            }
        });
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_PROTECTED_PDF)));
        QVERIFY(opened.wait(60'000));
        QVERIFY(reopened);
        QCOMPARE(opened.first().at(0).toUInt(), 3U);
        QCOMPARE(asks, 0);
    }

    void thePasswordFieldHidesWhatIsTypedAndForgetsIt() {
        vellora::PasswordDialog dialog(QStringLiteral("<b>a.pdf</b>"), 2, 3, true);
        QCOMPARE(dialog.field()->echoMode(), QLineEdit::Password);
        QVERIFY(dialog.field()->inputMethodHints().testFlag(Qt::ImhSensitiveData));
        // The file name is untrusted: shown as text, never as markup.
        QCOMPARE(dialog.message()->textFormat(), Qt::PlainText);
        QVERIFY(dialog.message()->text().contains(QStringLiteral("<b>a.pdf</b>")));
        QVERIFY(dialog.message()->text().contains(QStringLiteral("attempt 2 of 3")));
        dialog.field()->setText(QStringLiteral("secret"));
        QCOMPARE(dialog.takePassword(), QStringLiteral("secret"));
        QVERIFY(dialog.field()->text().isEmpty());
        QVERIFY(dialog.takePassword().isEmpty());

        vellora::PasswordDialog first(QStringLiteral("a.pdf"), 1, 3, false);
        QVERIFY(!first.message()->text().contains(QStringLiteral("not correct")));
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
