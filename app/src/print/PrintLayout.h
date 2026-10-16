// Where a page goes on the paper: the pure geometry of printing, apart from the engine and the
// printer so that it is tested with numbers.
#pragma once

#include <QList>
#include <QPointF>
#include <QSizeF>

namespace vellora {

enum class PrintScale {
    // Enlarge or shrink the page to fill the paper, keeping its shape.
    FitToPage,
    // One point of the page is one point on the paper. A page larger than the paper is cut off.
    ActualSize,
    // Actual size, but a page larger than the paper is shrunk to fit.
    ShrinkOversized,
};

enum class PageParity { All, Odd, Even };

struct PrintOptions {
    PrintScale scale = PrintScale::FitToPage;
    // Turn a landscape page on portrait paper (and the reverse) a quarter turn.
    bool autoRotate = true;
    // Centre the page on the paper; else it starts at the top left.
    bool center = true;
    bool grayscale = false;
    // The most dots per inch the page is rendered at (the printer may have more): 0 for no limit.
    // A document that allows only low-resolution printing is held to `kLowResolutionDpi`.
    int maxDpi = 0;
};

// What "low resolution" is for a document whose print permission says no to the full one
// (ISO 32000-2 table 22, bit 12): about the resolution of a fax-quality draft.
constexpr int kLowResolutionDpi = 150;

struct Placement {
    // The page is turned a quarter turn clockwise on the paper.
    bool rotated = false;
    // Device pixels of the printer per point of the page, after fitting.
    double scale = 1.0;
    // Pixels the engine renders per point of the page: `scale`, unless `maxDpi` holds it lower.
    double renderScale = 1.0;
    // The page on the paper in device pixels (already turned if `rotated`).
    QSizeF size;
    // The top left corner of that rectangle in the printable area.
    QPointF origin;
};

// Places a page of `page` points on a printable area of `printable` device pixels at `dpi` dots per
// inch. The sizes must be positive.
Placement placePage(QSizeF page, QSizeF printable, double dpi, const PrintOptions& options);

// The pages whose number (counted from 1) is odd or even, in their order.
QList<quint32> filterParity(const QList<quint32>& pages, PageParity parity);

// The pages in the order they are printed: reversed for a printer that stacks the last page first,
// and repeated `copies` times, whole document after whole document if `collate` else each page
// `copies` times in a row. `copies` below 1 is 1.
QList<quint32> sheetOrder(const QList<quint32>& pages, int copies, bool collate,
                          bool lastPageFirst);

} // namespace vellora
