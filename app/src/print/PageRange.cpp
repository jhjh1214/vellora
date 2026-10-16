#include "print/PageRange.h"

#include <QCoreApplication>
#include <QRegularExpression>
#include <QSet>
#include <QStringList>

namespace vellora {

namespace {

QString tr(const char* text) {
    return QCoreApplication::translate("PageRange", text);
}

constexpr QChar kEnDash(0x2013);

bool isHyphen(QChar c) {
    return c == QLatin1Char('-') || c == kEnDash;
}

} // namespace

PageRangeResult parsePageRanges(const QString& text, const PageResolver& resolve) {
    PageRangeResult result;
    QSet<quint32> seen;
    const auto add = [&](quint32 page) {
        if (!seen.contains(page)) {
            seen.insert(page);
            result.pages.append(page);
        }
    };

    bool any = false;
    const QStringList pieces = text.split(QRegularExpression(QStringLiteral("[,;]")));
    for (const QString& raw : pieces) {
        const QString piece = raw.trimmed();
        if (piece.isEmpty()) {
            continue;
        }
        any = true;
        if (const auto single = resolve(piece)) {
            add(*single);
            continue;
        }
        bool found = false;
        bool backwards = false;
        for (qsizetype i = 1; i + 1 < piece.size() && !found; ++i) {
            if (!isHyphen(piece[i])) {
                continue;
            }
            const auto first = resolve(piece.left(i).trimmed());
            const auto last = resolve(piece.mid(i + 1).trimmed());
            if (!first || !last) {
                continue;
            }
            if (*first > *last) {
                backwards = true;
                continue;
            }
            found = true;
            for (quint32 page = *first; page <= *last; ++page) {
                add(page);
            }
        }
        if (!found) {
            result.pages.clear();
            result.error =
                backwards
                    ? tr("“%1” runs backwards: the first page comes after the last.").arg(piece)
                    : tr("“%1” is not a page of this document.").arg(piece);
            return result;
        }
    }
    if (!any) {
        result.error = tr("Type the pages to print, for example 1-3, 7.");
    }
    return result;
}

} // namespace vellora
