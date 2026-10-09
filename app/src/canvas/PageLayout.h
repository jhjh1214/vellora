// Where each page sits, for any zoom, spread and view rotation. Pure geometry: no Qt widgets and no
// engine, so it is tested on its own and cheap enough to ask every frame.
//
// Pages are grouped into rows: one page each ("one-up"), or two side by side ("two-up", like the
// two pages of an open book, with the first page either on the left or, with a cover page, alone on
// the right). Rows are laid out top to bottom with a fixed gap (in logical pixels, not scaled by
// the zoom) above, between and below them. A page is `size * zoom` logical pixels; sizes are in PDF
// points and the view rotation turns every page by a multiple of 90 degrees (the pages in the file
// are never changed). A document of n pages costs one prefix sum of rows.
//
// Horizontal positions are relative to the centre line of the column (negative to the left), so the
// caller adds half its content width.
#pragma once

#include <QRectF>
#include <QSizeF>
#include <QVector>
#include <QtGlobal>

namespace vellora {

class PageLayout {
public:
    // Gap above the first row, between rows and below the last one, and between the two pages of a
    // spread, in logical pixels.
    static constexpr double kGap = 12.0;
    // The engine is a separate, possibly compromised process, so what it reports is bounded here:
    // at most this many pages (the layout costs 8 bytes a page), and only page sizes between 1 and
    // `kMaxPageDimension` points (Acrobat's own limit is 14,400) are believed.
    static constexpr quint32 kMaxPages = 1U << 22;
    static constexpr double kMaxPageDimension = 100'000.0;
    // Size of pages the engine has not told us about (US Letter), in points.
    static constexpr double kFallbackWidth = 612.0;
    static constexpr double kFallbackHeight = 792.0;

    // How pages are grouped into rows.
    enum class Spread {
        One,      // one page a row
        Two,      // pages (1, 2), (3, 4), ... side by side
        TwoCover, // page 1 alone on the right, then (2, 3), (4, 5), ...
    };

    // A place in the document that survives a change of zoom: a page and a distance from the top
    // edge of its row, in points.
    struct Anchor {
        quint32 page = 0;
        double offsetPoints = 0.0;
    };

    // `sizes` holds the pages whose size is known (a prefix of the document). The others, and any
    // whose size is not a sane finite size, take the size of the last sane known page, or Letter
    // if there is none. A page count above `kMaxPages` is cut to it.
    void setPages(quint32 pageCount, const QVector<QSizeF>& sizes);

    // The grouping and the view rotation in quarter turns clockwise (any integer; 0..3 after
    // reduction). Both change every position.
    void setArrangement(Spread spread, int rotation);
    Spread spread() const { return m_spread; }
    int rotation() const { return m_rotation; }

    quint32 pageCount() const { return m_pageCount; }
    // The size in the file (points), before the view rotation.
    QSizeF pageSize(quint32 page) const;
    // The size as shown: swapped when the view is turned a quarter.
    QSizeF displaySize(quint32 page) const;
    // The widest page as shown, in points.
    double maxPageWidth() const { return m_maxWidth; }

    // Rows.
    quint32 rowCount() const { return m_rowCount; }
    quint32 rowOf(quint32 page) const;
    quint32 firstPageOf(quint32 row) const;
    // 1 or 2 (0 past the end).
    quint32 pagesInRow(quint32 row) const;
    double rowTop(quint32 row, double zoom) const;
    double rowHeight(quint32 row, double zoom) const;
    // The row that holds the y coordinate (the last row starting at or above it; clamped).
    quint32 rowAt(double y, double zoom) const;
    // Rows that intersect [top, bottom): `first..first+count-1`; count is 0 when none do.
    struct Range {
        quint32 first = 0;
        quint32 count = 0;
    };
    Range rowsIn(double top, double bottom, double zoom) const;

    // A row's size in points as shown, and the part of its width that does not scale (the gap
    // between two pages): width on screen = `rowWidthPoints * zoom + rowFixedWidth`.
    double rowWidthPoints(quint32 row) const;
    double rowFixedWidth(quint32 row) const;
    double rowHeightPoints(quint32 row) const;

    // The rectangle of `page` in column coordinates: x relative to the centre line, y from the top
    // of the document.
    QRectF pageRect(quint32 page, double zoom) const;
    // Half the width the widest row needs, scaled and fixed part: `halfWidthPoints * zoom +
    // halfFixedWidth`.
    double halfWidthPoints() const { return m_halfWidthPoints; }
    double halfFixedWidth() const { return m_halfFixed; }

    // Page-oriented views of the rows (a page stands for its row; in a spread, for its first page).
    double pageTop(quint32 page, double zoom) const;
    double totalHeight(double zoom) const;
    quint32 pageAt(double y, double zoom) const;
    // The pages of the rows that intersect [top, bottom): `first..first+count-1`.
    Range pagesIn(double top, double bottom, double zoom) const;

    Anchor anchorAt(double y, double zoom) const;
    double yOf(const Anchor& anchor, double zoom) const;

    // Rectangles inside a page, in points of the page as it is in the file (origin top left) and in
    // pixels of the page as shown on screen (origin top left of the shown page), for a page of
    // `file` points turned `rotation` quarters clockwise at `zoom`. Each is the inverse of the
    // other.
    static QRectF toScreen(const QRectF& points, const QSizeF& file, int rotation, double zoom);
    static QRectF toPage(const QRectF& pixels, const QSizeF& file, int rotation, double zoom);

private:
    // 0 = left slot, 1 = right slot of the spread the page is in (the only slot for one-up).
    int slotOf(quint32 page) const;
    quint32 rowPages(quint32 row, quint32& first) const;
    void rebuild();

    quint32 m_pageCount = 0;
    quint32 m_rowCount = 0;
    Spread m_spread = Spread::One;
    int m_rotation = 0;
    QVector<QSizeF> m_known;
    QSizeF m_fallback{kFallbackWidth, kFallbackHeight};
    // m_heightBefore[i] is the summed height of rows 0..i-1, in points.
    QVector<double> m_heightBefore{0.0};
    double m_maxWidth = 0.0;
    double m_halfWidthPoints = 0.0;
    double m_halfFixed = 0.0;
};

} // namespace vellora