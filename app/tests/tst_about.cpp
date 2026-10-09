// The About dialog and the third-party licenses viewer.
#include "AboutDialog.h"
#include "BuildInfo.h"
#include "LicensesDialog.h"
#include "MainWindow.h"

#include <QFile>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QRegularExpression>
#include <QSet>
#include <QTest>
#include <QTextBlock>
#include <QTextCursor>
#include <algorithm>

class TstAbout : public QObject {
    Q_OBJECT

private slots:
    void buildInfoComesFromTheBuild() {
        const QString version = QLatin1String(vellora::buildinfo::kVersion);
        const QString commit = QLatin1String(vellora::buildinfo::kCommit);
        const QString date = QLatin1String(vellora::buildinfo::kBuildDate);
        QVERIFY(
            QRegularExpression(QStringLiteral("^\\d+\\.\\d+\\.\\d+")).match(version).hasMatch());
        QVERIFY(!commit.isEmpty());
        QVERIFY(
            QRegularExpression(QStringLiteral("^\\d{4}-\\d{2}-\\d{2}$")).match(date).hasMatch());
    }

    void aboutShowsVersionLicenseAndTheQtNotice() {
        vellora::AboutDialog about;
        const QString version = about.versionLabel()->text();
        QVERIFY2(version.contains(QLatin1String(vellora::buildinfo::kVersion)),
                 qPrintable(version));
        QVERIFY2(version.contains(QLatin1String(vellora::buildinfo::kCommit)), qPrintable(version));
        QVERIFY2(version.contains(QLatin1String(vellora::buildinfo::kBuildDate)),
                 qPrintable(version));

        QVERIFY(about.licenseLabel()->text().contains(QStringLiteral("MPL-2.0")));
        const QString qt = about.qtLabel()->text();
        QVERIFY(qt.contains(QStringLiteral("LGPL")));
        QVERIFY(qt.contains(QStringLiteral("replace")));
        QVERIFY(qt.contains(QString::fromLatin1(qVersion())));
        QVERIFY(qt.contains(QCoreApplication::applicationDirPath()));
        QVERIFY(qt.contains(QStringLiteral("https://code.qt.io/")));

        // Everything shown is plain text, whatever it contains.
        for (const QLabel* label : about.findChildren<QLabel*>()) {
            QCOMPARE(label->textFormat(), Qt::PlainText);
        }
    }

    void theLicenseTextIsEmbedded() {
        vellora::AboutDialog about;
        QDialog* dialog = about.showLicense();
        QVERIFY(dialog->isVisible());
        const auto* text = dialog->findChild<QPlainTextEdit*>();
        QVERIFY(text != nullptr);
        QVERIFY2(text->toPlainText().contains(QStringLiteral("Mozilla Public License")),
                 "the MPL text is not in the executable");
        dialog->close();
    }

    void theViewerListsEveryComponentOfTheLicensesFile() {
        QFile file(QStringLiteral(":/licenses/THIRD_PARTY_LICENSES"));
        QVERIFY(file.open(QIODevice::ReadOnly));
        const QString text = QString::fromUtf8(file.readAll());

        vellora::LicensesDialog dialog;
        const QStringList components = dialog.components();
        // Every crate the file credits is in the list...
        const QRegularExpression usedBy(QStringLiteral("^Used by:\\n((?:  - .*\\n)+)"),
                                        QRegularExpression::MultilineOption);
        const QRegularExpression crate(QStringLiteral("^  - (\\S+) (\\S+)"),
                                       QRegularExpression::MultilineOption);
        int crates = 0;
        for (auto block = usedBy.globalMatch(text); block.hasNext();) {
            for (auto line = crate.globalMatch(block.next().captured(1)); line.hasNext();) {
                const auto match = line.next();
                const QString name = match.captured(1) + QLatin1Char(' ') + match.captured(2);
                QVERIFY2(components.contains(name), qPrintable(name));
                ++crates;
            }
        }
        QVERIFY2(crates > 90, qPrintable(QString::number(crates)));
        // ...and so are the binary components, which are not crates.
        for (const char* name :
             {"PDFium", "Qt", "freetype (in PDFium)", "libjpeg-turbo (in PDFium)",
              "zlib (in PDFium)", "abseil (in PDFium)"}) {
            QVERIFY2(components.contains(QString::fromLatin1(name)), name);
        }
        for (const QString& crateName :
             {QStringLiteral("zeroize "), QStringLiteral("cxx "), QStringLiteral("stringprep ")}) {
            const bool any = std::any_of(components.begin(), components.end(),
                                         [&](const QString& c) { return c.startsWith(crateName); });
            QVERIFY2(any, qPrintable(crateName));
        }
        QCOMPARE(dialog.list()->count(), static_cast<int>(components.size()));
        QCOMPARE(
            components.size(),
            static_cast<qsizetype>(QSet<QString>(components.begin(), components.end()).size()));
    }

    void choosingAComponentShowsItsNotice() {
        vellora::LicensesDialog dialog;
        dialog.show();
        QVERIFY(dialog.select(QStringLiteral("PDFium")));
        QCOMPARE(dialog.viewer()->textCursor().block().text(), QStringLiteral("PDFium"));
        QVERIFY(dialog.select(QStringLiteral("libjpeg-turbo (in PDFium)")));
        QVERIFY(dialog.viewer()->textCursor().block().text().startsWith(
            QStringLiteral("--- libjpeg_turbo")));
        const QString crate = *std::find_if(
            dialog.components().begin(), dialog.components().end(),
            [](const QString& c) { return c.startsWith(QStringLiteral("zeroize ")); });
        QVERIFY(dialog.select(crate));
        QVERIFY(dialog.viewer()->textCursor().block().text().contains(crate));
        QVERIFY(!dialog.select(QStringLiteral("not a component")));

        // Filtering narrows the list.
        dialog.filter()->setText(QStringLiteral("PDFIUM"));
        QStringList visible = dialog.visibleComponents();
        QVERIFY(visible.contains(QStringLiteral("PDFium")));
        QVERIFY(visible.size() > 10 && visible.size() < dialog.components().size());
        dialog.filter()->setText(QStringLiteral("zzzz"));
        QVERIFY(dialog.visibleComponents().isEmpty());
        dialog.filter()->clear();
        QCOMPARE(dialog.visibleComponents(), dialog.components());
    }

    void aMissingLicenseFileIsSaidNotHidden() {
        vellora::LicensesDialog dialog(nullptr, QStringLiteral(":/licenses/nope"));
        QVERIFY(dialog.components().isEmpty());
        QVERIFY(dialog.viewer()->toPlainText().contains(QStringLiteral("missing")));
    }

    void helpAboutIsACommandThatOpensTheDialog() {
        vellora::MainWindow window;
        window.show();
        const vellora::Command* about = window.commands().find(QStringLiteral("help.about"));
        QVERIFY(about != nullptr);
        QCOMPARE(about->title, QStringLiteral("About Vellora"));
        QVERIFY(window.commands().run(QStringLiteral("help.about")));
        const auto* dialog = window.findChild<vellora::AboutDialog*>();
        QVERIFY(dialog != nullptr);
        QVERIFY(dialog->isVisible());
        QVERIFY(dialog->thirdPartyButton() != nullptr);
        // The button opens the viewer.
        const_cast<vellora::AboutDialog*>(dialog)->thirdPartyButton()->click();
        const auto* licenses = dialog->findChild<vellora::LicensesDialog*>();
        QVERIFY(licenses != nullptr);
        QVERIFY(licenses->isVisible());
    }
};

QTEST_MAIN(TstAbout)
#include "tst_about.moc"