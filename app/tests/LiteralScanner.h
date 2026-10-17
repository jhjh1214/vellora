// Finds user-visible string literals that skipped `tr()` (M1 task 22). Standard C++ only, so the
// scanner can be tried without Qt; `tst_untranslated_literals` runs it over `app/src`.
//
// It is a tokenizer plus a stack of the calls a literal sits in, not a C++ parser. A literal is
// reported when it contains words and either
//   1. it is an argument of a function that puts text in front of the user (`setText`,
//      `setWindowTitle`, `QLabel(...)`, ...), see `kSinks`, or
//   2. it reads like a sentence (two or more words) and is not inside `tr()`, `translate()`,
//      `QT_TR_NOOP()`, a log call or an assertion.
// Text that is not for the user (the issue template of a crash report, a hidden command-line flag,
// a token read from a file) is exempt with a comment `// i18n-ok: <reason>` on the line of the
// literal or one of the 3 lines above it.
#pragma once

#include <algorithm>
#include <cctype>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace literalscan {

struct Finding {
    int line = 0;
    std::string literal; // the text, as in the source (escapes not decoded)
    std::string call;    // the function the literal is an argument of
};

namespace detail {

enum class Kind { Ident, String, Punct };

struct Token {
    Kind kind = Kind::Punct;
    std::string text;
    int line = 1;
};

inline bool isIdentStart(char c) {
    return std::isalpha(static_cast<unsigned char>(c)) != 0 || c == '_';
}
inline bool isIdentChar(char c) {
    return std::isalnum(static_cast<unsigned char>(c)) != 0 || c == '_';
}

// A prefix of a string literal: `u8"..."`, `L"..."`, `R"x(...)x"`.
inline bool isStringPrefix(std::string_view s) {
    return s == "u8" || s == "u" || s == "U" || s == "L" || s == "R" || s == "u8R" || s == "uR" ||
           s == "UR" || s == "LR";
}

struct Lexed {
    std::vector<Token> tokens;
    std::set<int> markerLines; // lines with `i18n-ok`
};

inline Lexed lex(std::string_view src) {
    Lexed out;
    int line = 1;
    bool lineStart = true; // only whitespace so far on this line
    for (size_t i = 0; i < src.size();) {
        const char c = src[i];
        if (c == '\n') {
            ++line;
            lineStart = true;
            ++i;
        } else if (std::isspace(static_cast<unsigned char>(c)) != 0) {
            ++i;
        } else if (c == '/' && i + 1 < src.size() && src[i + 1] == '/') {
            const size_t end = std::min(src.find('\n', i), src.size());
            if (src.substr(i, end - i).find("i18n-ok") != std::string_view::npos) {
                out.markerLines.insert(line);
            }
            i = end;
        } else if (c == '/' && i + 1 < src.size() && src[i + 1] == '*') {
            size_t end = src.find("*/", i + 2);
            end = end == std::string_view::npos ? src.size() : end + 2;
            if (src.substr(i, end - i).find("i18n-ok") != std::string_view::npos) {
                out.markerLines.insert(line);
            }
            line += static_cast<int>(std::count(src.begin() + i, src.begin() + end, '\n'));
            i = end;
        } else if (c == '#' && lineStart) {
            // A preprocessor line, with its continuations.
            while (i < src.size() && src[i] != '\n') {
                if (src[i] == '\\' && i + 1 < src.size() && src[i + 1] == '\n') {
                    ++line;
                    ++i;
                }
                ++i;
            }
        } else if (c == '"') {
            // The prefix, if any, was lexed as an identifier just before.
            std::string raw;
            const bool isRaw = !out.tokens.empty() && out.tokens.back().kind == Kind::Ident &&
                               out.tokens.back().text.back() == 'R' &&
                               isStringPrefix(out.tokens.back().text) &&
                               out.tokens.back().line == line && i > 0 && src[i - 1] == 'R';
            if (isRaw) {
                const size_t open = src.find('(', i);
                const std::string delim(src.substr(i + 1, open - i - 1));
                const std::string close = ")" + delim + "\"";
                size_t end = src.find(close, open);
                end = end == std::string_view::npos ? src.size() : end;
                raw = std::string(src.substr(open + 1, end - open - 1));
                out.tokens.pop_back();
                out.tokens.push_back({Kind::String, raw, line});
                line += static_cast<int>(std::count(raw.begin(), raw.end(), '\n'));
                i = std::min(end + close.size(), src.size());
            } else {
                if (!out.tokens.empty() && out.tokens.back().kind == Kind::Ident &&
                    isStringPrefix(out.tokens.back().text) && i > 0 && isIdentChar(src[i - 1])) {
                    out.tokens.pop_back();
                }
                std::string text;
                size_t j = i + 1;
                while (j < src.size() && src[j] != '"' && src[j] != '\n') {
                    if (src[j] == '\\' && j + 1 < src.size()) {
                        text += src[j++];
                    }
                    text += src[j++];
                }
                out.tokens.push_back({Kind::String, text, line});
                i = j < src.size() ? j + 1 : j;
            }
        } else if (c == '\'') {
            // A character literal (or a digit separator, which follows a digit).
            if (i > 0 && std::isdigit(static_cast<unsigned char>(src[i - 1])) != 0) {
                ++i;
            } else {
                size_t j = i + 1;
                while (j < src.size() && src[j] != '\'' && src[j] != '\n') {
                    j += src[j] == '\\' ? 2 : 1;
                }
                i = std::min(j + 1, src.size());
            }
        } else if (isIdentStart(c)) {
            size_t j = i;
            while (j < src.size() && isIdentChar(src[j])) {
                ++j;
            }
            out.tokens.push_back({Kind::Ident, std::string(src.substr(i, j - i)), line});
            i = j;
        } else if (std::isdigit(static_cast<unsigned char>(c)) != 0) {
            size_t j = i;
            while (j < src.size() && (isIdentChar(src[j]) || src[j] == '.' || src[j] == '\'')) {
                ++j;
            }
            i = j;
        } else {
            out.tokens.push_back({Kind::Punct, std::string(1, c), line});
            ++i;
        }
        if (c != '\n' && std::isspace(static_cast<unsigned char>(c)) == 0) {
            lineStart = false;
        }
    }
    // Adjacent literals are one literal ("a" "b").
    std::vector<Token> merged;
    for (Token& t : out.tokens) {
        if (t.kind == Kind::String && !merged.empty() && merged.back().kind == Kind::String) {
            merged.back().text += t.text;
        } else {
            merged.push_back(std::move(t));
        }
    }
    out.tokens = std::move(merged);
    return out;
}

// Functions whose string argument the user reads.
inline const std::set<std::string>& sinks() {
    static const std::set<std::string> kSinks = {
        "setText",
        "setWindowTitle",
        "setToolTip",
        "setStatusTip",
        "setWhatsThis",
        "setPlaceholderText",
        "setAccessibleName",
        "setAccessibleDescription",
        "setTitle",
        "setLabelText",
        "setInformativeText",
        "setDetailedText",
        "setButtonText",
        "setTabText",
        "setTabToolTip",
        "addTab",
        "addItem",
        "addAction",
        "addMenu",
        "addRow",
        "setHeaderLabels",
        "setHeaderLabel",
        "setPrefix",
        "setSuffix",
        "setSpecialValueText",
        "showMessage",
        "setPlainText",
        "setHtml",
        "appendPlainText",
        "insertItem",
        "setNameFilter",
        "setNameFilters",
        "setCheckboxText",
        "QLabel",
        "QPushButton",
        "QCheckBox",
        "QRadioButton",
        "QGroupBox",
        "QAction",
        "QMenu",
        "QToolButton",
        "QTreeWidgetItem",
        "QListWidgetItem",
        "QStandardItem",
        "QMessageBox",
        "warning",
        "information",
        "critical",
        "question",
        "getOpenFileName",
        "getOpenFileNames",
        "getSaveFileName",
        "getText",
        "getInt",
        "getItem",
        "setCaption",
        "setOkButtonText",
    };
    return kSinks;
}

// Calls whose literals are not shown to the user.
inline const std::set<std::string>& exempt() {
    static const std::set<std::string> kExempt = {
        "tr",
        "translate",
        "QT_TR_NOOP",
        "QT_TRANSLATE_NOOP",
        "QT_TR_NOOP_UTF8",
        "qsTr",
        "qDebug",
        "qInfo",
        "qWarning",
        "qCritical",
        "qFatal",
        "qCDebug",
        "qCInfo",
        "qCWarning",
        "qCCritical",
        "Q_ASSERT",
        "Q_ASSERT_X",
        "Q_LOGGING_CATEGORY",
        "QLoggingCategory",
        "static_assert",
        "QVERIFY",
        "QVERIFY2",
        "QCOMPARE",
        "QTEST",
        "QFAIL",
        "QSKIP",
        "Q_DECLARE_LOGGING_CATEGORY",
        "Q_PROPERTY",
        "Q_CLASSINFO",
    };
    return kExempt;
}

// Wrappers that make a QString of a literal; they are not the consumer of the text.
inline bool isWrapper(const std::string& name) {
    return name == "QStringLiteral" || name == "QLatin1String" || name == "QLatin1StringView" ||
           name == "QString" || name == "fromUtf8" || name == "fromLatin1" ||
           name == "QStringView" || name == "QByteArray" || name == "QUtf8StringView" ||
           name == "QAnyStringView" || name == "QLatin1Char" || name == "QStringList" ||
           name == "arg" || name == "QKeySequence" || name == "QStringLiteralView";
}

// The words of the text: pieces between spaces (or `\n` escapes) that are two or more letters,
// with at most punctuation around them. Identifiers (`view.zoomIn`, `--flag`), placeholders and
// paths are not words, so a settings key is not mistaken for a sentence.
inline int wordCount(const std::string& text) {
    std::string s;
    for (size_t i = 0; i < text.size(); ++i) {
        if (text[i] == '\\' && i + 1 < text.size() && (text[i + 1] == 'n' || text[i + 1] == 't')) {
            s += ' ';
            ++i;
        } else {
            s += text[i];
        }
    }
    int words = 0;
    size_t i = 0;
    while (i < s.size()) {
        while (i < s.size() && std::isspace(static_cast<unsigned char>(s[i])) != 0) {
            ++i;
        }
        const size_t start = i;
        while (i < s.size() && std::isspace(static_cast<unsigned char>(s[i])) == 0) {
            ++i;
        }
        std::string piece = s.substr(start, i - start);
        const std::string_view trim = "\"'(),.:;!?";
        while (!piece.empty() && trim.find(piece.front()) != std::string_view::npos) {
            piece.erase(0, 1);
        }
        while (!piece.empty() && trim.find(piece.back()) != std::string_view::npos) {
            piece.pop_back();
        }
        size_t letters = 0;
        bool plain = !piece.empty();
        // UTF-8: accented Latin letters (U+00C0 to U+02AF) are letters; symbols such as an arrow
        // (a glyph on a button) are not.
        bool inLatinLetter = false;
        for (const char ch : piece) {
            const auto c = static_cast<unsigned char>(ch);
            bool letter = std::isalpha(c) != 0;
            if (c >= 0xC3 && c <= 0xCA) {
                inLatinLetter = letter = true;
            } else if (inLatinLetter && c >= 0x80 && c < 0xC0) {
                inLatinLetter = false; // the continuation byte of the letter just counted
                letter = true;
                --letters;
            } else {
                inLatinLetter = false;
            }
            letters += letter ? 1 : 0;
            plain = plain && (letter || ch == '\'' || ch == '-');
        }
        if (plain && letters >= 2) {
            ++words;
        }
    }
    return words;
}

} // namespace detail

/// The literals in `source` that look user-visible and are not translated.
inline std::vector<Finding> findUntranslated(std::string_view source) {
    using namespace detail;
    const Lexed lexed = lex(source);
    const auto& toks = lexed.tokens;
    std::vector<Finding> found;
    std::vector<std::string> frames; // the function name of every open `(`
    for (size_t i = 0; i < toks.size(); ++i) {
        const Token& t = toks[i];
        if (t.kind == Kind::Punct && t.text == "(") {
            frames.push_back(i > 0 && toks[i - 1].kind == Kind::Ident ? toks[i - 1].text : "");
        } else if (t.kind == Kind::Punct && t.text == ")") {
            if (!frames.empty()) {
                frames.pop_back();
            }
        } else if (t.kind == Kind::String) {
            if (std::any_of(frames.begin(), frames.end(),
                            [](const std::string& f) { return exempt().count(f) > 0; })) {
                continue;
            }
            bool marked = false;
            for (int l = t.line - 3; l <= t.line; ++l) {
                marked = marked || lexed.markerLines.count(l) > 0;
            }
            if (marked) {
                continue;
            }
            std::string call;
            for (auto f = frames.rbegin(); f != frames.rend(); ++f) {
                if (!isWrapper(*f)) {
                    call = *f;
                    break;
                }
            }
            const int words = wordCount(t.text);
            const bool sink = sinks().count(call) > 0 && words >= 1;
            // A `case`-label or comparison with an empty call name is not a sentence for the user
            // unless it reads like one.
            if (sink || words >= 2) {
                found.push_back({t.line, t.text, call});
            }
        }
    }
    return found;
}

} // namespace literalscan
