// Where each page sits in the continuous vertical column, for any zoom. Pure geometry: no Qt
// widgets and no engine, so it is tested on its own and cheap enough to ask every frame.
//
// Pages are laid out top to bottom with a fixed gap (in logical pixels, not scaled by the zoom)
// above, between and below them. A page is `size * zoom` logical pixels. Sizes are in PDF points.
// A document of n pages costs one prefix sum of n + 1 doubles.
#pragma once

#include <QSizeF>
#include <QVector>
#include <QtGlobal>

namespace vellora {

class PageLayout {
public:
    // Gap above the first page, between pages and below the last one, in logical pixels.
    static constexpr double kGap = 12.0;
    // The engine is a separate, possibly compromised process, so what it reports is bounded here:
    // at most this many pages (the layout costs 8 bytes a page), and only page sizes between 1 and
    // `kMaxPageDimension` points (Acrobat's own limit is 14,400) are believed.
    static constexpr quint32 kMaxPages = 1U << 22;
    static constexpr double kMaxPageDimension = 100'000.0;
    // Size of pages the engine has not told us about (US Letter), in points.
    static constexpr double kFallbackWidth = 612.0;
    static constexpr double kFallbackHeight = 792.0;

    // A place in the document that survives a change of zoom: a page and a distance from its top
    // edge, in points.
    struct Anchor {
        quint32 page = 0;
        double offsetPoints = 0.0;
    };

    // `sizes` holds the pages whose size is known (a prefix of the document). The others, and any
    // whose size is not a sane finite size, take the size of the last sane known page, or Letter
    // if there is none. A page count above `kMaxPages` is cut to it.
    void setPages(quint32 pageCount, const QVector<QSizeF>& sizes);

    quint32 pageCount() const { return m_pageCount; }
    QSizeF pageSize(quint32 page) const;
    // The widest page, in points.
    double maxPageWidth() const { return m_maxWidth; }

    // Top edge of `page` and the full height of the column, in logical pixels.
    double pageTop(quint32 page, double zoom) const;
    double totalHeight(double zoom) const;

    // The last page whose top edge is at or above `y`; clamped to the document.
    quint32 pageAt(double y, double zoom) const;
    // The pages that intersect [top, bottom): `first..last` inclusive. Both 0 for an empty
    // document, and `count` is 0 when nothing intersects.
    struct Range {
        quint32 first = 0;
        quint32 count = 0;
    };
    Range pagesIn(double top, double bottom, double zoom) const;

    Anchor anchorAt(double y, double zoom) const;
    double yOf(const Anchor& anchor, double zoom) const;

private:
    quint32 m_pageCount = 0;
    QVector<QSizeF> m_known;
    QSizeF m_fallback{kFallbackWidth, kFallbackHeight};
    // m_heightBefore[i] is the summed height of pages 0..i-1, in points.
    QVector<double> m_heightBefore{0.0};
    double m_maxWidth = 0.0;
};

} // namespace vellora
