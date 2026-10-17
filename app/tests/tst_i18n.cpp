// The pseudo-locales (accented text about 30% longer, and a right-to-left layout) and what they
// show of the real windows. With VELLORA_SCREENSHOT_DIR set, the test also writes PNGs of the
// windows in English, in the pseudo-locale and in the pseudo-RTL locale (CI uploads them).
#include "AboutDialog.h"
#include "I18n.h"
#include "MainWindow.h"
#include "PasswordDialog.h"
#include "links/UriConfirmDialog.h"

#include <QAction>
#include <QCoreApplication>
#include <QDir>
#include <QGuiApplication>
#include <QImage>
#include <QMenu>
#include <QMenuBar>
#include <QSet>
#include <QSignalSpy>
#include <QTest>

namespace i18n = vellora::i18n;

namespace {

// All the actions with text in a menu, submenus included.
void collectActions(const QMenu* menu, QStringList* texts) {
    for (const QAction* action : menu->actions()) {
        if (action->isSeparator() || action->text().isEmpty()) {
            continue;
        }
        texts->append(action->text());
        if (action->menu() != nullptr) {
            collectActions(action->menu(), texts);
        }
    }
}

bool isPseudo(const QString& text) {
    // `[` opens every pseudo-localised string; right-to-left ones start with U+202B before it.
    return text.startsWith(QLatin1Char('[')) || text.startsWith(QChar(0x202B));
}

} // namespace

class TstI18n : public QObject {
    Q_OBJECT

    QTranslator* m_installed = nullptr;

    void useLocale(const QString& value) {
        removeLocale();
        QString name;
        const i18n::Mode mode = i18n::parseMode(value, &name);
        i18n::install(*QCoreApplication::instance(), mode, name);
        const auto translators = QCoreApplication::instance()->findChildren<QTranslator*>();
        m_installed = translators.isEmpty() ? nullptr : translators.last();
    }

    void removeLocale() {
        if (m_installed != nullptr) {
            QCoreApplication::removeTranslator(m_installed);
            delete m_installed;
            m_installed = nullptr;
        }
        QGuiApplication::setLayoutDirection(Qt::LeftToRight);
    }

    static void save(const QPixmap& pixmap, const QString& locale, const QString& name) {
        const QString dir = qEnvironmentVariable("VELLORA_SCREENSHOT_DIR");
        if (dir.isEmpty()) {
            return;
        }
        QVERIFY2(QDir().mkpath(dir), qPrintable(dir));
        const QString path = QDir(dir).filePath(QStringLiteral("%1-%2.png").arg(locale, name));
        QVERIFY2(pixmap.save(path), qPrintable(path));
    }

private slots:
    void cleanup() { removeLocale(); }

    void parsesTheEnvironmentValue() {
        QString name;
        QCOMPARE(i18n::parseMode(QString(), &name), i18n::Mode::System);
        QCOMPARE(i18n::parseMode(QStringLiteral("  "), &name), i18n::Mode::System);
        QCOMPARE(i18n::parseMode(QStringLiteral("pseudo"), &name), i18n::Mode::Pseudo);
        QCOMPARE(i18n::parseMode(QStringLiteral("PSEUDO-RTL"), &name), i18n::Mode::PseudoRtl);
        QCOMPARE(i18n::parseMode(QStringLiteral(" de_DE "), &name), i18n::Mode::Named);
        QCOMPARE(name, QStringLiteral("de_DE"));
    }

    void pseudoTextIsAccentedAndAboutThirtyPercentLonger() {
        const QString source = QStringLiteral("Open the document");
        const QString pseudo = i18n::pseudoLocalize(source, false);
        QVERIFY(pseudo.startsWith(QLatin1Char('[')) && pseudo.endsWith(QLatin1Char(']')));
        QVERIFY2(pseudo.size() >= source.size() * 13 / 10, qPrintable(pseudo));
        QVERIFY2(pseudo.size() <= source.size() * 2, qPrintable(pseudo));
        for (const QChar c : pseudo) {
            // Every letter is accented: no plain ASCII letter is left.
            QVERIFY2(!(c.unicode() < 0x80 && c.isLetter()), qPrintable(pseudo));
        }
        // Every ASCII letter of the alphabet has a look-alike of the same length.
        const QString alphabet =
            QStringLiteral("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");
        const QString accentedAlphabet = i18n::pseudoLocalize(alphabet, false);
        QVERIFY(accentedAlphabet.startsWith(QLatin1Char('[')));
        QCOMPARE(accentedAlphabet.mid(1, alphabet.size()).size(), alphabet.size());
        QCOMPARE(QSet<QChar>(accentedAlphabet.cbegin() + 1,
                             accentedAlphabet.cbegin() + 1 + alphabet.size())
                     .size(),
                 alphabet.size());
    }

    void pseudoTextKeepsWhatCodeReads() {
        const QString text = i18n::pseudoLocalize(
            QStringLiteral("Page %1 of %L2, %n item(s) &Save && <b>bold</b> &amp; more"), false);
        QVERIFY2(text.contains(QStringLiteral("%1")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("%L2")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("%n")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("<b>")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("</b>")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("&amp;")), qPrintable(text));
        QVERIFY2(text.contains(QStringLiteral("&&")), qPrintable(text));
        // The mnemonic is still one `&` in front of a (now accented) letter.
        QVERIFY2(text.contains(QStringLiteral(u"&Š")), qPrintable(text));
        QCOMPARE(text.count(QLatin1Char('&')), 4); // &Š, &&, &amp;
        // Text that is not text stays as it is.
        QCOMPARE(i18n::pseudoLocalize(QString(), false), QString());
        QCOMPARE(i18n::pseudoLocalize(QStringLiteral(" "), false), QStringLiteral(" "));
        // A formatted result is still formatted.
        QCOMPARE(i18n::pseudoLocalize(QStringLiteral("%1 / %2"), false)
                     .arg(QStringLiteral("3"), QStringLiteral("9"))
                     .count(QLatin1Char('3')),
                 1);
    }

    void pseudoRtlTextIsEmbeddedRightToLeft() {
        const QString text = i18n::pseudoLocalize(QStringLiteral("Close"), true);
        QCOMPARE(text.front(), QChar(0x202B));
        QCOMPARE(text.back(), QChar(0x202C));
    }

    void theTranslatorAnswersEveryString() {
        useLocale(QStringLiteral("pseudo"));
        const QString shown = QCoreApplication::translate("anything", "Go to page");
        QVERIFY2(isPseudo(shown), qPrintable(shown));
        QVERIFY(QObject::tr("Open…") != QStringLiteral("Open…"));
        removeLocale();
        QCOMPARE(QCoreApplication::translate("anything", "Go to page"),
                 QStringLiteral("Go to page"));
    }

    // Everything in the menus is translated, including what the code builds at run time (the
    // untranslated-literal test sees only the sources).
    void everyMenuTextIsTranslated() {
        useLocale(QStringLiteral("pseudo"));
        vellora::MainWindow window;
        QStringList texts;
        for (const QAction* action : window.menuBar()->actions()) {
            texts.append(action->text());
            if (action->menu() != nullptr) {
                collectActions(action->menu(), &texts);
            }
        }
        QVERIFY(texts.size() > 20);
        QStringList plain;
        for (const QString& text : std::as_const(texts)) {
            // A mnemonic can come first: "&File" is shown as "&[...]" only when written "[&..".
            if (!isPseudo(text)) {
                plain.append(text);
            }
        }
        QVERIFY2(plain.isEmpty(), qPrintable(plain.join(QStringLiteral(" | "))));
        QVERIFY(isPseudo(window.windowTitle()));
    }

    void rightToLeftMirrorsTheWindow() {
        // Left to right: "File" is the first menu, at the left of the menu bar.
        {
            vellora::MainWindow window;
            window.resize(900, 600);
            window.show();
            QCOMPARE(window.layoutDirection(), Qt::LeftToRight);
            const QMenuBar* bar = window.menuBar();
            const int x = bar->actionGeometry(bar->actions().first()).center().x();
            QVERIFY2(x < bar->width() / 2, qPrintable(QString::number(x)));
        }
        useLocale(QStringLiteral("pseudo-rtl"));
        QCOMPARE(QGuiApplication::layoutDirection(), Qt::RightToLeft);
        vellora::MainWindow window;
        window.resize(900, 600);
        window.show();
        QCOMPARE(window.layoutDirection(), Qt::RightToLeft);
        const QMenuBar* bar = window.menuBar();
        const int x = bar->actionGeometry(bar->actions().first()).center().x();
        QVERIFY2(x > bar->width() / 2, qPrintable(QString::number(x)));
    }

    void screenshots_data() {
        QTest::addColumn<QString>("locale");
        QTest::newRow("en") << QStringLiteral("en");
        QTest::newRow("pseudo") << QStringLiteral("pseudo");
        QTest::newRow("pseudo-rtl") << QStringLiteral("pseudo-rtl");
    }

    // The windows a person sees first, in each locale. Written only when VELLORA_SCREENSHOT_DIR is
    // set; otherwise it still builds them, which proves they open in every locale.
    void screenshots() {
        QFETCH(QString, locale);
        useLocale(locale);
        const QString& tag = locale;

        vellora::MainWindow window;
        window.resize(1100, 760);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY2(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)),
                 qPrintable(window.documentStatus()));
        QVERIFY(opened.wait(60'000));
        QTest::qWait(300); // the thumbnails and the status bar settle
        save(window.grab(), tag, QStringLiteral("main-window"));

        if (QMenu* file = window.menuBar()->actions().first()->menu()) {
            file->popup(window.mapToGlobal(QPoint(20, 30)));
            QVERIFY(QTest::qWaitForWindowExposed(file));
            save(file->grab(), tag, QStringLiteral("file-menu"));
            file->close();
        }

        vellora::AboutDialog about(&window);
        about.show();
        QVERIFY(QTest::qWaitForWindowExposed(&about));
        save(about.grab(), tag, QStringLiteral("about"));
        about.close();

        vellora::PasswordDialog password(QStringLiteral("report.pdf"), 2, 3, true, &window);
        password.show();
        QVERIFY(QTest::qWaitForWindowExposed(&password));
        save(password.grab(), tag, QStringLiteral("password"));
        password.close();

        vellora::UriConfirmDialog uri(QStringLiteral("https://example.com/a/long/path?x=1"),
                                      QStringLiteral("example.com"), &window);
        uri.show();
        QVERIFY(QTest::qWaitForWindowExposed(&uri));
        save(uri.grab(), tag, QStringLiteral("link-confirmation"));
        uri.close();
    }
};

QTEST_MAIN(TstI18n)
#include "tst_i18n.moc"
