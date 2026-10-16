// What a reader asks to print, and what the prompts are told about the document. Plain data, so a
// test can stand in for the dialogs.
#pragma once

#include "print/PageRange.h"
#include "print/PrintLayout.h"

#include <QList>
#include <QPrinter>
#include <memory>

namespace vellora {

// What the print dialogs need to know about the open document.
struct PrintContext {
    quint32 pageCount = 0;
    // The page on screen, zero-based.
    quint32 currentPage = 0;
    // Names a page by its label or number (for the page range).
    PageResolver resolve;
    // The page range text to start from, usually "1-<pageCount>" in the document's own labels.
    QString defaultRange;
    // The document allows printing at low resolution only (ISO 32000-2 table 22, bit 12).
    bool lowResolutionOnly = false;
    // Asked for a print preview rather than a print.
    bool preview = false;
};

// An accepted print dialog: where to print, which pages (in print order, copies and page order
// already applied) and how.
struct PrintRequest {
    std::shared_ptr<QPrinter> printer;
    // Zero-based pages in the order they are printed (see `sheetOrder`).
    QList<quint32> sheets;
    PrintOptions options;
};

} // namespace vellora
