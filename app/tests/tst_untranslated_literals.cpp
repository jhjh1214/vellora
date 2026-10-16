// No user-visible string literal in `app/src` skips `tr()` (M1 task 22). The scanner is
// `LiteralScanner.h` (what it reports and how to exempt text: its header). It is plain C++ over the
// source files, so the check needs no build of the shell and no engine.
#include "LiteralScanner.h"

#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QTest>

namespace {

using literalscan::findUntranslated;

QStringList callsOf(const char* source) {
    QStringList calls;
    for (const auto& finding : findUntranslated(source)) {
        calls.append(QString::fromStdString(finding.call));
    }
    return calls;
}

} // namespace

class TstUntranslatedLiterals : public QObject {
    Q_OBJECT

private slots:
    // The sources: every .cpp and .h under app/src.
    void appSourcesHaveNoUntranslatedLiterals() {
        const QString root = QStringLiteral(VELLORA_APP_SRC_DIR);
        QStringList problems;
        int files = 0;
        QDirIterator it(root, {QStringLiteral("*.cpp"), QStringLiteral("*.h")}, QDir::Files,
                        QDirIterator::Subdirectories);
        while (it.hasNext()) {
            const QString path = it.next();
            QFile file(path);
            QVERIFY2(file.open(QIODevice::ReadOnly), qPrintable(path));
            const QByteArray bytes = file.readAll();
            ++files;
            for (const auto& f : findUntranslated(
                     std::string_view(bytes.constData(), static_cast<size_t>(bytes.size())))) {
                problems.append(QStringLiteral("%1:%2: %3(\"%4\")")
                                    .arg(QDir(root).relativeFilePath(path))
                                    .arg(f.line)
                                    .arg(QString::fromStdString(f.call.empty() ? "?" : f.call),
                                         QString::fromStdString(f.literal)));
            }
        }
        // A wrong path must not pass by scanning nothing.
        QVERIFY2(files > 40,
                 qPrintable(QStringLiteral("only %1 files under %2").arg(files).arg(root)));
        QVERIFY2(problems.isEmpty(),
                 qPrintable(QStringLiteral("user-visible text without tr() (wrap it in tr(), or "
                                           "mark it `// i18n-ok: <reason>`, see "
                                           "app/tests/LiteralScanner.h):\n") +
                            problems.join(QLatin1Char('\n'))));
    }

    // The scanner itself: what it must report ...
    void reportsLiteralsGivenToWidgets() {
        QCOMPARE(callsOf(R"(void f() { label->setText(QStringLiteral("Open file")); })"),
                 QStringList{QStringLiteral("setText")});
        QCOMPARE(callsOf(R"(void f() { setWindowTitle("Vellora"); })"),
                 QStringList{QStringLiteral("setWindowTitle")});
        QCOMPARE(callsOf(R"(void f() { auto* l = new QLabel(QLatin1String("Name"), this); })"),
                 QStringList{QStringLiteral("QLabel")});
        QCOMPARE(callsOf(R"(void f() { button->setToolTip(u"Close"); })").size(), 1);
        QCOMPARE(callsOf(R"(void f() { menu->addAction(QStringLiteral("Copy")); })").size(), 1);
    }

    void reportsSentencesAnywhere() {
        QCOMPARE(callsOf(R"(void f() { show(QStringLiteral("This file is damaged.")); })").size(),
                 1);
        QCOMPARE(callsOf(R"(const char* kMessage = "Something went wrong";)").size(), 1);
        QCOMPARE(callsOf(R"(void f() { x("first part of " "the sentence"); })").size(), 1);
        QCOMPARE(callsOf(R"(void f() { x(R"t(raw text for you)t"); })").size(), 1);
    }

    // ... and what it must let through.
    void acceptsTranslatedText() {
        QVERIFY(callsOf(R"(void f() { label->setText(tr("Open file")); })").isEmpty());
        QVERIFY(
            callsOf(R"(void f() { setText(QObject::tr("Open the file %1").arg(n)); })").isEmpty());
        QVERIFY(
            callsOf(R"(QString g() { return QCoreApplication::translate("Ctx", "Not found"); })")
                .isEmpty());
        QVERIFY(callsOf(R"(const Row kRows[] = {{"id", QT_TR_NOOP("A label shown to you")}};)")
                    .isEmpty());
    }

    void acceptsIdentifiersGlyphsAndLogs() {
        QVERIFY(callsOf(R"(void f() { run(QStringLiteral("view.zoomIn")); })").isEmpty());
        QVERIFY(callsOf(R"(void f() { s.value(QStringLiteral("window/geometry")); })").isEmpty());
        QVERIFY(callsOf(R"(void f() { button->setText(QStringLiteral("▲")); })").isEmpty());
        QVERIFY(callsOf(R"(void f() { qWarning("cannot write the file: %s", p); })").isEmpty());
        QVERIFY(callsOf(R"(void f() { Q_ASSERT_X(ok, "f", "the value is not set"); })").isEmpty());
        QVERIFY(callsOf(R"(#include "some header with spaces.h")").isEmpty());
        QVERIFY(callsOf(R"(void f() { char c = '"'; setText(tr("Hello world")); })").isEmpty());
    }

    void honoursTheExemptionComment() {
        QVERIFY(callsOf("void f() {\n  // i18n-ok: a log line\n  x(\"some words here\");\n}")
                    .isEmpty());
        QVERIFY(
            callsOf("void f() {\n  x(\"some words here\"); // i18n-ok: a log line\n}").isEmpty());
        // It reaches 3 lines, not further.
        QCOMPARE(
            callsOf("// i18n-ok: far above\n\n\n\n\nvoid f() { x(\"some words here\"); }").size(),
            1);
    }

    void ignoresCommentsAndStringsWithQuotes() {
        QVERIFY(callsOf("// setText(\"Not code at all\")\nvoid f() {}").isEmpty());
        QVERIFY(callsOf("/* setText(\"Not code at all\") */ void f() {}").isEmpty());
        QCOMPARE(callsOf(R"(void f() { setText("say \"hello there\" now"); })").size(), 1);
    }
};

QTEST_APPLESS_MAIN(TstUntranslatedLiterals)
#include "tst_untranslated_literals.moc"
