// Translations: which language the application shows, and the pseudo-locales that test layouts.
//
// All user-visible text goes through `tr()` (or `QCoreApplication::translate` outside a QObject).
// English is the source language: the shipped `vellora_en.ts` has no translations, and without a
// matching `vellora_<language>.qm` the source strings are shown. `tests/tst_untranslated_literals`
// finds literals that skipped `tr()`.
//
// The language is chosen once, in `main`, before any window exists:
//   VELLORA_LOCALE=pseudo      every string accented, about 30% longer and in [brackets], to find
//                              layouts that clip and text that skipped `tr()`
//   VELLORA_LOCALE=pseudo-rtl  the same, with a right-to-left layout, to check mirroring
//   VELLORA_LOCALE=<name>      e.g. `de`, `ar_EG`: loads that language's .qm
//   (unset)                    the system's UI languages
#pragma once

#include <QString>
#include <QTranslator>

class QCoreApplication;

namespace vellora::i18n {

/// What `VELLORA_LOCALE` asks for.
enum class Mode {
    System,    ///< unset or empty: the user's UI languages
    Pseudo,    ///< `pseudo`
    PseudoRtl, ///< `pseudo-rtl`
    Named,     ///< any other value: that language
};

/// Parses the value of `VELLORA_LOCALE`. For `Named`, `name` is the trimmed value.
[[nodiscard]] Mode parseMode(const QString& value, QString* name = nullptr);

/// `text` as the pseudo-locale shows it: letters accented, about 30% longer, in brackets.
/// Placeholders (`%1`, `%n`, `%L2`), mnemonics (`&&`), HTML tags and entities are left alone. With
/// `rtl` the result is wrapped in a right-to-left embedding. Blank text stays as it is.
[[nodiscard]] QString pseudoLocalize(const QString& text, bool rtl);

/// A translator that answers every string with its pseudo-localised form.
class PseudoTranslator : public QTranslator {
public:
    explicit PseudoTranslator(bool rtl, QObject* parent = nullptr)
        : QTranslator(parent), m_rtl(rtl) {}

    bool isEmpty() const override { return false; }
    QString translate(const char* context, const char* sourceText, const char* disambiguation,
                      int n) const override;

private:
    bool m_rtl;
};

/// Installs the translator for `mode` (and, for `PseudoRtl`, the right-to-left layout) on the
/// application. Returns a note for the log: what was installed, or why nothing was.
QString install(QCoreApplication& app, Mode mode, const QString& name);

/// `install` with the mode taken from the environment variable `VELLORA_LOCALE`.
QString installFromEnvironment(QCoreApplication& app);

} // namespace vellora::i18n
