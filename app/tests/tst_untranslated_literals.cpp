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

std::vector<literalscan::Finding> scan(const QByteArray& bytes) {
    return findUntranslated(std::string_view(bytes.constData(), static_cast<size_t>(bytes.size())));
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
            ++files;
            for (const auto& f : scan(file.readAll())) {
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

    // The scanner itself, on snippets. They are plain strings in a table, not macro arguments:
    // moc's preprocessor cannot read a raw string inside one.
    void scannerFindsWhatItShould_data() {
        QTest::addColumn<QString>("source");
        QTest::addColumn<int>("count");
        QTest::addColumn<QString>("call"); // the call of the first finding, if any

        // What it must report ...
        QTest::newRow("setText")
            << QString("void f() { label->setText(QStringLiteral(\"Open file\")); }") << 1
            << QStringLiteral("setText");
        QTest::newRow("setWindowTitle") << QString("void f() { setWindowTitle(\"Vellora\"); }") << 1
                                        << QStringLiteral("setWindowTitle");
        QTest::newRow("QLabel")
            << QString("void f() { auto* l = new QLabel(QLatin1String(\"Name\"), this); }") << 1
            << QStringLiteral("QLabel");
        QTest::newRow("u prefix") << QString("void f() { button->setToolTip(u\"Close\"); }") << 1
                                  << QStringLiteral("setToolTip");
        QTest::newRow("addAction")
            << QString("void f() { menu->addAction(QStringLiteral(\"Copy\")); }") << 1
            << QStringLiteral("addAction");
        QTest::newRow("sentence in a call")
            << QString("void f() { show(QStringLiteral(\"This file is damaged.\")); }") << 1
            << QStringLiteral("show");
        QTest::newRow("sentence in a constant")
            << QString("const char* kMessage = \"Something went wrong\";") << 1
            << QStringLiteral("");
        QTest::newRow("concatenated")
            << QString("void f() { x(\"first part of \" \"the sentence\"); }") << 1
            << QStringLiteral("x");
        QTest::newRow("raw string")
            << QString("void f() { x(R\"t(raw text for you)t\"); }") << 1 << QStringLiteral("x");
        QTest::newRow("escaped quotes")
            << QString("void f() { setText(\"say \\\"hello there\\\" now\"); }") << 1
            << QStringLiteral("setText");

        // ... and what it must let through.
        QTest::newRow("tr") << QString("void f() { label->setText(tr(\"Open file\")); }") << 0
                            << QStringLiteral("");
        QTest::newRow("QObject::tr with arg")
            << QString("void f() { setText(QObject::tr(\"Open the file %1\").arg(n)); }") << 0
            << QStringLiteral("");
        QTest::newRow("translate")
            << QString(
                   "QString g() { return QCoreApplication::translate(\"Ctx\", \"Not found\"); }")
            << 0 << QStringLiteral("");
        QTest::newRow("QT_TR_NOOP")
            << QString("const Row kRows[] = {{\"id\", QT_TR_NOOP(\"A label shown to you\")}};") << 0
            << QStringLiteral("");
        QTest::newRow("command id") << QString("void f() { run(QStringLiteral(\"view.zoomIn\")); }")
                                    << 0 << QStringLiteral("");
        QTest::newRow("settings key")
            << QString("void f() { s.value(QStringLiteral(\"window/geometry\")); }") << 0
            << QStringLiteral("");
        QTest::newRow("glyph") << QString("void f() { button->setText(QStringLiteral(\"▲\")); }")
                               << 0 << QStringLiteral("");
        QTest::newRow("log") << QString("void f() { qWarning(\"cannot write the file: %s\", p); }")
                             << 0 << QStringLiteral("");
        QTest::newRow("assert")
            << QString("void f() { Q_ASSERT_X(ok, \"f\", \"the value is not set\"); }") << 0
            << QStringLiteral("");
        QTest::newRow("include") << QString("#include \"some header with spaces.h\"") << 0
                                 << QStringLiteral("");
        QTest::newRow("char literal")
            << QString("void f() { char c = '\"'; setText(tr(\"Hello world\")); }") << 0
            << QStringLiteral("");
        QTest::newRow("marker above")
            << QString("void f() {\n  // i18n-ok: a log line\n  x(\"some words here\");\n}") << 0
            << QStringLiteral("");
        QTest::newRow("marker on the line")
            << QString("void f() {\n  x(\"some words here\"); // i18n-ok: a log line\n}") << 0
            << QStringLiteral("");
        QTest::newRow("marker too far above")
            << QString("// i18n-ok: far above\n\n\n\n\nvoid f() { x(\"some words here\"); }") << 1
            << QStringLiteral("x");
        QTest::newRow("line comment")
            << QString("// setText(\"Not code at all\")\nvoid f() {}") << 0 << QStringLiteral("");
        QTest::newRow("block comment")
            << QString("/* setText(\"Not code at all\") */ void f() {}") << 0 << QStringLiteral("");
    }

    void scannerFindsWhatItShould() {
        QFETCH(QString, source);
        QFETCH(int, count);
        QFETCH(QString, call);
        const auto findings = scan(source.toUtf8());
        QCOMPARE(static_cast<int>(findings.size()), count);
        if (count > 0) {
            QCOMPARE(QString::fromStdString(findings.front().call), call);
        }
    }
};

QTEST_APPLESS_MAIN(TstUntranslatedLiterals)
#include "tst_untranslated_literals.moc"
