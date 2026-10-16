// M1 task 18c: the Inspector panel and the JSON export, with a real engine. The panel shows the
// report the engine makes (the text `vellora inspect --json` prints; the Rust tests prove they are
// the same), so what is checked here is that the panel shows every part of that report, in plain
// text, and that the export writes it as it is.
#include "MainWindow.h"
#include "VelloraTestMain.h"
#include "sidebar/InspectorView.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QTreeWidget>

using vellora::DocumentTab;
using vellora::InspectorView;
using Tab = vellora::ThumbnailSidebar::Tab;

namespace {

constexpr int kWaitMs = 60'000;

// Two pages (200 x 100 and 100 x 200 points); the catalog runs a script on open and has two
// embedded files; the first page has two links to addresses; the information dictionary has a
// title with markup in it, which must be shown as it is.
QByteArray reportedPdf(bool damaged = false) {
    const QStringList objects = {
        QStringLiteral("<< /Type /Catalog /Pages 2 0 R /OpenAction 6 0 R /Names << /EmbeddedFiles "
                       "7 0 R >> >>"),
        QStringLiteral("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
        QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Annots [8 0 R 9 0 R] "
                       ">>"),
        QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>"),
        QStringLiteral("<< /Title (<b>Quarterly</b> report) /Author (A. Writer) /Producer (Vellora "
                       "tests) /CreationDate (D:20260102030405Z) >>"),
        QStringLiteral("<< /S /JavaScript /JS (app.alert\\(1\\)) >>"),
        QStringLiteral("<< /Names [(notes.txt) 10 0 R (data.csv) 10 0 R] >>"),
        QStringLiteral("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /A << /S /URI /URI "
                       "(https://example.org/one) >> >>"),
        QStringLiteral("<< /Type /Annot /Subtype /Link /Rect [20 0 30 10] /A << /S /URI /URI "
                       "(mailto:someone@example.org) >> >>"),
        QStringLiteral("<< >>"),
    };
    QByteArray out = "%PDF-1.7\n";
    QList<qsizetype> offsets;
    for (int i = 0; i < objects.size(); ++i) {
        offsets << out.size();
        out += QStringLiteral("%1 0 obj\n%2\nendobj\n").arg(i + 1).arg(objects.at(i)).toUtf8();
    }
    const qsizetype xref = out.size();
    out += QStringLiteral("xref\n0 %1\n0000000000 65535 f \n").arg(objects.size() + 1).toUtf8();
    for (const qsizetype offset : offsets) {
        out += QStringLiteral("%1 00000 n \n").arg(offset, 10, 10, QLatin1Char('0')).toUtf8();
    }
    // A wrong startxref makes the engine rebuild the table, which is a repair to report.
    out += QStringLiteral("trailer\n<< /Size %1 /Root 1 0 R /Info 5 0 R >>\nstartxref\n%2\n%%EOF\n")
               .arg(objects.size() + 1)
               .arg(damaged ? xref + 7 : xref)
               .toUtf8();
    return out;
}

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    DocumentTab* tab = nullptr;
    QStringList notices;

    explicit Fixture(const QByteArray& pdf, const QString& name = QStringLiteral("report.pdf")) {
        QFile file(dir.filePath(name));
        if (!file.open(QIODevice::WriteOnly)) {
            qFatal("cannot write the test document");
        }
        file.write(pdf);
        file.close();
        window.resize(1000, 800);
        window.show();
        tab = &window.currentTab();
        tab->setNotifier([this](const QString& text) { notices.append(text); });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(dir.filePath(name)) || !opened.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
    }

    InspectorView& inspector() { return *tab->sidebar().inspector(); }

    // Shows the Info tab and waits for the report.
    void show() {
        tab->showInspector();
        if (!QTest::qWaitFor([this] { return inspector().isLoaded(); }, kWaitMs)) {
            qFatal("the report did not arrive");
        }
    }
};

QJsonObject parse(const QString& json) {
    return QJsonDocument::fromJson(json.toUtf8()).object();
}

} // namespace

class TstInspector : public QObject {
    Q_OBJECT

private slots:
    void theReportIsAskedForWhenTheTabIsShownAndKeptAsTheEngineSentIt() {
        Fixture f(reportedPdf());
        QVERIFY(f.tab->sidebar().inspector() != nullptr);
        // Nothing is asked for until the reader looks.
        QVERIFY(!f.inspector().isLoaded() && !f.inspector().isLoading());

        QStringList sent;
        connect(&f.window.session(), &vellora::EngineSession::summaryReady, this,
                [&](quint64, const QString& json) { sent.append(json); });
        f.tab->sidebar().setCurrentTab(Tab::Inspector);
        QTRY_VERIFY_WITH_TIMEOUT(f.inspector().isLoaded(), kWaitMs);
        QCOMPARE(sent.size(), 1);
        QCOMPARE(f.inspector().json(), sent.first());
        // It is what the command line prints: pretty JSON of the report, readable as JSON.
        QVERIFY(f.inspector().json().startsWith(QStringLiteral("{\n  \"file_size\": ")));
        QCOMPARE(f.inspector().report(), parse(sent.first()));

        // Showing the tab again does not ask again.
        f.tab->sidebar().setCurrentTab(Tab::Thumbnails);
        f.tab->sidebar().setCurrentTab(Tab::Inspector);
        QTest::qWait(200);
        QCOMPARE(sent.size(), 1);
    }

    void thePropertiesShowWhatTheReportSays() {
        Fixture f(reportedPdf());
        f.show();
        const QJsonObject r = f.inspector().report();
        InspectorView& v = f.inspector();

        QCOMPARE(v.valueOf("version"), QStringLiteral("1.7"));
        QCOMPARE(v.valueOf("pages"), QString::number(r["pages"].toInteger()));
        QCOMPARE(v.valueOf("pages"), QStringLiteral("2"));
        QVERIFY(v.valueOf("file_size").contains(QString::number(r["file_size"].toInteger())));
        // The sizes, each with its page count, smaller first when the counts are equal.
        const QJsonArray sizes = r["page_sizes"].toArray();
        QCOMPARE(sizes.size(), 2);
        for (int i = 0; i < sizes.size(); ++i) {
            const QString shown = v.valueOf(QStringLiteral("page_size.%1").arg(i));
            QVERIFY2(shown.contains(QString::number(sizes[i]["width"].toDouble())),
                     qPrintable(shown));
            QVERIFY2(shown.contains(QString::number(sizes[i]["height"].toDouble())),
                     qPrintable(shown));
        }
        QVERIFY(v.find(QStringLiteral("page_size.2")) == nullptr);
        // The information, as text, with markup left as it is.
        QCOMPARE(v.valueOf("info.title"), QStringLiteral("<b>Quarterly</b> report"));
        QCOMPARE(v.valueOf("info.author"), QStringLiteral("A. Writer"));
        QCOMPARE(v.valueOf("info.producer"), QStringLiteral("Vellora tests"));
        QCOMPARE(v.valueOf("info.creation_date"), QStringLiteral("D:20260102030405Z"));
        QVERIFY(v.find(QStringLiteral("info.subject")) == nullptr);
        QVERIFY(v.find(QStringLiteral("info.modification_date")) == nullptr);
        QCOMPARE(v.valueOf("tagged"), QStringLiteral("no"));
        QCOMPARE(v.valueOf("linearized"), QStringLiteral("no"));
    }

    void theSecurityPartNamesTheRiskyFeaturesFoundAndExplainsEach() {
        Fixture f(reportedPdf());
        f.show();
        InspectorView& v = f.inspector();
        const QJsonObject features = v.report()["features"].toObject();

        // Exactly the features the report marks, no more and no fewer.
        QStringList expected;
        for (auto it = features.begin(); it != features.end(); ++it) {
            if (it.key() != QLatin1String("complete") && it.value().toBool()) {
                expected.append(it.key());
            }
        }
        expected.sort();
        QStringList shown = v.shownFeatures();
        shown.sort();
        QCOMPARE(shown, expected);
        QVERIFY(shown.contains(QStringLiteral("open_action_javascript")));
        QVERIFY(shown.contains(QStringLiteral("javascript_actions")));
        QVERIFY(shown.contains(QStringLiteral("uri_actions")));
        QVERIFY(shown.contains(QStringLiteral("embedded_files")));
        QVERIFY(!shown.contains(QStringLiteral("acroform")));
        QVERIFY(v.find(QStringLiteral("feature.none")) == nullptr);

        // Each says what it is in plain words, in its tool tip.
        for (const QString& feature : shown) {
            const QTreeWidgetItem* row = v.find(QStringLiteral("feature.") + feature);
            QVERIFY(row != nullptr);
            QVERIFY2(!InspectorView::explanation(feature).isEmpty(), qPrintable(feature));
            QCOMPARE(row->toolTip(0), InspectorView::explanation(feature));
        }
        QVERIFY(InspectorView::explanation(QStringLiteral("open_action_javascript"))
                    .contains(QStringLiteral("never runs")));

        // The addresses and the file names, all of them.
        QCOMPARE(v.valueOf("uris"), QStringLiteral("2"));
        QCOMPARE(v.find(QStringLiteral("uris.0"))->text(0),
                 QStringLiteral("https://example.org/one"));
        QCOMPARE(v.find(QStringLiteral("uris.1"))->text(0),
                 QStringLiteral("mailto:someone@example.org"));
        QCOMPARE(v.valueOf("embedded_files"), QStringLiteral("2"));
        QCOMPARE(v.find(QStringLiteral("embedded_files.0"))->text(0), QStringLiteral("notes.txt"));
        QCOMPARE(v.find(QStringLiteral("embedded_files.1"))->text(0), QStringLiteral("data.csv"));
        QCOMPARE(v.valueOf("encryption"), QStringLiteral("none"));
    }

    void aPlainDocumentSaysNoRiskyFeaturesWereFound() {
        QByteArray plain = reportedPdf();
        plain.replace("/OpenAction 6 0 R ", "");
        plain.replace("/Names << /EmbeddedFiles 7 0 R >>", "");
        plain.replace("/Annots [8 0 R 9 0 R]", "");
        // The offsets moved; the engine repairs the table, which is fine for this test.
        Fixture f(plain);
        f.show();
        InspectorView& v = f.inspector();
        QVERIFY(v.shownFeatures().isEmpty());
        QCOMPARE(v.valueOf("feature.none"), QStringLiteral("none found"));
        QVERIFY(v.find(QStringLiteral("uris")) == nullptr);
        QVERIFY(v.find(QStringLiteral("embedded_files")) == nullptr);
    }

    void anEncryptedDocumentShowsItsPermissions() {
        Fixture f([] {
            QFile file(QStringLiteral(VELLORA_ENCRYPTION_DIR "/r6-aes-256-no-print.pdf"));
            if (!file.open(QIODevice::ReadOnly)) {
                qFatal("cannot read the fixture");
            }
            return file.readAll();
        }());
        f.show();
        InspectorView& v = f.inspector();
        const QJsonObject e = v.report()["encryption"].toObject();
        QVERIFY2(v.valueOf("encryption").contains(QStringLiteral("Standard")),
                 qPrintable(v.valueOf("encryption")));
        QVERIFY(
            v.valueOf("encryption").contains(QStringLiteral("%1-bit").arg(e["key_bits"].toInt())));
        QVERIFY(v.valueOf("encryption.unlocked_with").contains(e["unlocked_with"].toString()));
        const QJsonObject allowed = e["allowed"].toObject();
        for (const QString& key : allowed.keys()) {
            const QString shown = v.valueOf(QStringLiteral("permissions.") + key);
            QCOMPARE(shown, allowed[key].toBool() ? QStringLiteral("yes") : QStringLiteral("no"));
        }
        // This fixture does not let the user print.
        QCOMPARE(v.valueOf("permissions.print"), QStringLiteral("no"));
        QCOMPARE(v.valueOf("permissions.print_high_quality"), QStringLiteral("no"));
        QCOMPARE(v.valueOf("permissions.mask"), QString::number(e["permissions"].toInt()));
    }

    void revisionsAreListedWithTheirByteRanges() {
        Fixture f(reportedPdf());
        f.show();
        InspectorView& v = f.inspector();
        const QJsonArray ranges = v.report()["revision_ranges"].toArray();
        QVERIFY(!ranges.isEmpty());
        QCOMPARE(v.valueOf("revisions"), QString::number(v.report()["revisions"].toInteger()));
        for (const QJsonValue& value : ranges) {
            const QJsonObject range = value.toObject();
            const QString shown =
                v.valueOf(QStringLiteral("revision.%1").arg(range["number"].toInteger()));
            QCOMPARE(shown, QStringLiteral("bytes %1 to %2")
                                .arg(range["start"].toInteger())
                                .arg(range["end"].toInteger()));
        }
    }

    void repairsAreListedWithTheirReasons() {
        Fixture f(reportedPdf(false));
        f.show();
        QCOMPARE(f.inspector().valueOf("repaired"), QStringLiteral("no"));
        QVERIFY(f.inspector().find(QStringLiteral("repair.0")) == nullptr);

        Fixture damaged(reportedPdf(true), QStringLiteral("damaged.pdf"));
        damaged.show();
        InspectorView& v = damaged.inspector();
        QCOMPARE(v.valueOf("repaired"), QStringLiteral("yes"));
        const QJsonArray reasons = v.report()["repair_reasons"].toArray();
        QVERIFY(!reasons.isEmpty());
        for (qsizetype i = 0; i < reasons.size(); ++i) {
            QCOMPARE(v.find(QStringLiteral("repair.%1").arg(i))->text(0), reasons[i].toString());
        }
    }

    void theExportWritesTheEnginesTextAsItIs() {
        Fixture f(reportedPdf());
        QString suggested;
        const QString target = f.dir.filePath(QStringLiteral("out.json"));
        f.tab->setExportChooser([&](const QString& name) {
            suggested = name;
            return target;
        });
        // Asked for before the report is here: it is fetched, then saved.
        QVERIFY(!f.inspector().isLoaded());
        f.tab->exportReport();
        QTRY_VERIFY_WITH_TIMEOUT(QFile::exists(target), kWaitMs);
        QCOMPARE(suggested, QStringLiteral("report.report.json"));

        QFile file(target);
        QVERIFY(file.open(QIODevice::ReadOnly));
        const QByteArray written = file.readAll();
        QVERIFY(f.inspector().isLoaded());
        // The text of `vellora inspect --json`, whose last character is the newline println adds.
        QCOMPARE(written, f.inspector().json().toUtf8() + '\n');
        QVERIFY(!QJsonDocument::fromJson(written).isNull());

        // A second export overwrites it without asking the engine again.
        QFile::remove(target);
        f.tab->exportReport();
        QVERIFY(QFile::exists(target));
    }

    void cancellingTheExportWritesNothing() {
        Fixture f(reportedPdf());
        int asked = 0;
        f.tab->setExportChooser([&](const QString&) {
            ++asked;
            return QString();
        });
        f.show();
        f.tab->exportReport();
        QCOMPARE(asked, 1);
        QCOMPARE(QDir(f.dir.path()).entryList({QStringLiteral("*.json")}).size(), 0);
    }

    void aReportThatCannotBeSavedIsSaidSo() {
        Fixture f(reportedPdf());
        f.show();
        f.tab->setExportChooser([&](const QString&) {
            return f.dir.filePath(QStringLiteral("no/such/folder/out.json"));
        });
        f.tab->exportReport();
        QCOMPARE(f.notices.size(), 1);
        QVERIFY2(f.notices.first().contains(QStringLiteral("could not be saved")),
                 qPrintable(f.notices.first()));
    }

    void anotherDocumentForgetsTheReport() {
        Fixture f(reportedPdf());
        f.show();
        QCOMPARE(f.inspector().valueOf("pages"), QStringLiteral("2"));

        QTemporaryDir other;
        QFile file(other.filePath(QStringLiteral("small.pdf")));
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(
            "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type "
            "/Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R "
            "/MediaBox [0 0 300 300] >>\nendobj\ntrailer\n<< /Size 4 /Root 1 0 R >>\n%%EOF\n");
        file.close();
        QSignalSpy opened(&f.window.session(), &vellora::EngineSession::opened);
        QVERIFY(f.tab->open(other.filePath(QStringLiteral("small.pdf"))));
        QVERIFY(opened.wait(kWaitMs));
        QVERIFY(!f.inspector().isLoaded());
        QCOMPARE(f.inspector().tree().topLevelItemCount(), 0);
        f.show();
        QCOMPARE(f.inspector().valueOf("pages"), QStringLiteral("1"));
        QVERIFY(f.inspector().find(QStringLiteral("info.title")) == nullptr);
    }

    void theCommandsShowTheTabAndSaveTheReport() {
        Fixture f(reportedPdf());
        QVERIFY(f.window.commands().find(QStringLiteral("view.inspector")) != nullptr);
        QVERIFY(f.window.commands().find(QStringLiteral("file.exportReport")) != nullptr);
        f.tab->setSidebarVisible(false);
        QVERIFY(f.window.commands().run(QStringLiteral("view.inspector")));
        QVERIFY(f.tab->sidebarVisible());
        QCOMPARE(f.tab->sidebar().currentTab(), Tab::Inspector);
        QTRY_VERIFY_WITH_TIMEOUT(f.inspector().isLoaded(), kWaitMs);
    }

    void toolTipsMadeOfTheFilesTextAreEscapedSoTheyAreNotReadAsMarkup() {
        Fixture f(reportedPdf());
        f.show();
        const QTreeWidgetItem* title = f.inspector().find(QStringLiteral("info.title"));
        QVERIFY(title != nullptr);
        // The title holds `<b>Quarterly</b>`: in the tool tip it is text, not bold.
        for (const int column : {0, 1}) {
            const QString tip = title->toolTip(column);
            QVERIFY2(tip.contains(QStringLiteral("&lt;b&gt;Quarterly&lt;/b&gt;")), qPrintable(tip));
            QVERIFY2(!tip.contains(QStringLiteral("<b>")), qPrintable(tip));
        }
        // The addresses too, whatever they hold.
        const QTreeWidgetItem* address = f.inspector().find(QStringLiteral("uris.0"));
        QVERIFY(address != nullptr);
        QVERIFY(!address->toolTip(0).contains(QLatin1Char('<')) ||
                address->toolTip(0).startsWith(QStringLiteral("<p style='white-space:pre'>")));
    }

    void eachRowIsPlainTextWhateverTheFileSays() {
        Fixture f(reportedPdf());
        f.show();
        // Markup in the title is not interpreted: the item holds the text, nothing rich.
        const QTreeWidgetItem* title = f.inspector().find(QStringLiteral("info.title"));
        QVERIFY(title != nullptr);
        QCOMPARE(title->data(1, Qt::DisplayRole).toString(),
                 QStringLiteral("<b>Quarterly</b> report"));
        QCOMPARE(title->data(1, Qt::DisplayRole).metaType().id(), QMetaType::QString);
    }
};

VELLORA_TEST_MAIN(TstInspector)
#include "tst_inspector.moc"
