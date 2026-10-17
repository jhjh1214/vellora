#include "I18n.h"

#include <QCoreApplication>
#include <QGuiApplication>
#include <QLocale>
#include <QtGlobal>
#include <algorithm>
#include <cmath>

namespace vellora::i18n {

namespace {

// ASCII letters to look-alikes with diacritics (each is one UTF-16 unit, so lengths stay honest).
QChar accented(QChar c) {
    static const QString from =
        QStringLiteral("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ");
    static const QString to =
        QStringLiteral(u"åƀçďéƒĝĥîĵķļɱñöþɋŕšţûṽŵẋýžÅƁÇĐÉƑĜĤÎĴĶĹṀÑÖÞɊŔŠŢÛṼŴẊÝŽ");
    const qsizetype at = from.indexOf(c);
    return at < 0 ? c : to.at(at);
}

bool isAsciiLetter(QChar c) {
    return (c >= QLatin1Char('a') && c <= QLatin1Char('z')) ||
           (c >= QLatin1Char('A') && c <= QLatin1Char('Z'));
}

// The length of a `%1`, `%L2`, `%n` placeholder starting at `i`, or 0.
qsizetype placeholderLength(const QString& s, qsizetype i) {
    if (s.at(i) != QLatin1Char('%')) {
        return 0;
    }
    qsizetype j = i + 1;
    if (j < s.size() && s.at(j) == QLatin1Char('L')) {
        ++j;
    }
    if (j < s.size() && s.at(j) == QLatin1Char('n')) {
        return j + 1 - i;
    }
    const qsizetype digits = j;
    while (j < s.size() && s.at(j).isDigit()) {
        ++j;
    }
    return j > digits ? j - i : 0;
}

// The length of an HTML tag (`<b>`) or entity (`&amp;`) starting at `i`, or 0.
qsizetype markupLength(const QString& s, qsizetype i) {
    if (s.at(i) == QLatin1Char('<')) {
        const qsizetype end = s.indexOf(QLatin1Char('>'), i);
        // Only what looks like a tag: `a < b` stays text.
        if (end > i + 1 && (isAsciiLetter(s.at(i + 1)) || s.at(i + 1) == QLatin1Char('/'))) {
            return end + 1 - i;
        }
    } else if (s.at(i) == QLatin1Char('&')) {
        qsizetype j = i + 1;
        while (j < s.size() &&
               (isAsciiLetter(s.at(j)) || s.at(j).isDigit() || s.at(j) == QLatin1Char('#'))) {
            ++j;
        }
        if (j > i + 1 && j < s.size() && s.at(j) == QLatin1Char(';')) {
            return j + 1 - i;
        }
    }
    return 0;
}

} // namespace

Mode parseMode(const QString& value, QString* name) {
    const QString v = value.trimmed();
    if (name != nullptr) {
        *name = v;
    }
    if (v.isEmpty()) {
        return Mode::System;
    }
    if (v.compare(QLatin1String("pseudo"), Qt::CaseInsensitive) == 0) {
        return Mode::Pseudo;
    }
    if (v.compare(QLatin1String("pseudo-rtl"), Qt::CaseInsensitive) == 0) {
        return Mode::PseudoRtl;
    }
    return Mode::Named;
}

QString pseudoLocalize(const QString& text, bool rtl) {
    if (text.trimmed().isEmpty()) {
        return text; // nothing to read: spaces and empty strings stay as they are
    }
    QString out;
    out.reserve(text.size() * 2 + 8);
    for (qsizetype i = 0; i < text.size();) {
        const qsizetype keep = std::max(placeholderLength(text, i), markupLength(text, i));
        if (keep > 0) {
            out += QStringView(text).mid(i, keep);
            i += keep;
        } else if (text.at(i) == QLatin1Char('&') && i + 1 < text.size() &&
                   text.at(i + 1) == QLatin1Char('&')) {
            out += QLatin1String("&&"); // a literal ampersand in a menu text
            i += 2;
        } else {
            out += accented(text.at(i)); // also the letter after a mnemonic `&`
            ++i;
        }
    }
    // About 30% longer: padding after the text.
    const auto padding = static_cast<qsizetype>(std::ceil(static_cast<double>(text.size()) * 0.3));
    out += QLatin1Char(' ');
    out += QString(padding, QChar(0x00B7));
    // The brackets show where a string starts and ends, so clipping is visible and a string
    // without them was not translated.
    out = QLatin1Char('[') + out + QLatin1Char(']');
    if (rtl) {
        // U+202B right-to-left embedding ... U+202C pop directional formatting.
        out = QChar(0x202B) + out + QChar(0x202C);
    }
    return out;
}

QString PseudoTranslator::translate(const char* context, const char* sourceText,
                                    const char* disambiguation, int n) const {
    Q_UNUSED(context);
    Q_UNUSED(disambiguation);
    Q_UNUSED(n);
    if (sourceText == nullptr) {
        return {};
    }
    return pseudoLocalize(QString::fromUtf8(sourceText), m_rtl);
}

QString install(QCoreApplication& app, Mode mode, const QString& name) {
    switch (mode) {
    case Mode::System:
    case Mode::Named: {
        // `vellora_<language>.qm` from the executable's resources (qt_add_translations). For
        // English, or a language without a file, the source strings stay.
        auto* translator = new QTranslator(&app);
        const QLocale locale = mode == Mode::Named ? QLocale(name) : QLocale::system();
        if (translator->load(locale, QStringLiteral("vellora"), QStringLiteral("_"),
                             QStringLiteral(":/i18n"))) {
            app.installTranslator(translator);
            return QStringLiteral("translation %1").arg(locale.name());
        }
        delete translator;
        // i18n-ok: the notes are for the log
        return QStringLiteral("no translation for %1: English").arg(locale.name());
    }
    case Mode::Pseudo:
    case Mode::PseudoRtl: {
        const bool rtl = mode == Mode::PseudoRtl;
        app.installTranslator(new PseudoTranslator(rtl, &app));
        if (rtl) {
            QGuiApplication::setLayoutDirection(Qt::RightToLeft);
        }
        // i18n-ok: for the log
        return rtl ? QStringLiteral("pseudo-locale, right-to-left")
                   : QStringLiteral("pseudo-locale");
    }
    }
    return {};
}

QString installFromEnvironment(QCoreApplication& app) {
    QString name;
    const Mode mode = parseMode(qEnvironmentVariable("VELLORA_LOCALE"), &name);
    return install(app, mode, name);
}

} // namespace vellora::i18n
