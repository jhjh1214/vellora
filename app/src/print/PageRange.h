// The pages a reader asks to print, typed as text: "1-3, 7", page labels ("iv", "A-3") and ranges
// between them ("ii-v"). A pure function of the text and of how the document names its pages, so
// it is tested without a document.
#pragma once

#include <QList>
#include <QString>
#include <functional>
#include <optional>

namespace vellora {

struct PageRangeResult {
    // Zero-based page indices in the order typed, each page once.
    QList<quint32> pages;
    // Empty if `text` was understood; else a sentence for the reader naming what was not.
    QString error;

    bool ok() const { return error.isEmpty(); }
};

// Finds the page a word names: a label of the document, else a page number. None if there is no
// such page.
using PageResolver = std::function<std::optional<quint32>(const QString& text)>;

// Reads comma- or semicolon-separated pieces, each one page or a range `first-last` (a hyphen or
// a dash). A piece that is a page by itself wins over a split at a hyphen, so the label "A-3"
// works; otherwise each hyphen is tried as the split, so "A-3-A-5" works too. A range runs from
// its first page to its last, and must not run backwards. An empty text is an error.
PageRangeResult parsePageRanges(const QString& text, const PageResolver& resolve);

} // namespace vellora
