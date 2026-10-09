#include "canvas/PageLayout.h"

#include <algorithm>
#include <cmath>

namespace vellora {

namespace {

bool isSane(const QSizeF& size) {
    const auto sane = [](double side) {
        return std::isfinite(side) && side >= 1.0 && side <= PageLayout::kMaxPageDimension;
    };
    return sane(size.width()) && sane(size.height());
}

int quarterTurns(int rotation) {
    return ((rotation % 4) + 4) % 4;
}

quint32 rowsFor(quint32 pages, PageLayout::Spread spread) {
    switch (spread) {
    case PageLayout::Spread::One:
        return pages;
    case PageLayout::Spread::Two:
        return (pages + 1) / 2;
    case PageLayout::Spread::TwoCover:
        return pages == 0 ? 0 : 1 + pages / 2;
    }
    return pages;
}

} // namespace

void PageLayout::setPages(quint32 pageCount, const QVector<QSizeF>& sizes) {
    pageCount = std::min(pageCount, kMaxPages);
    m_pageCount = pageCount;
    m_known = sizes;
    if (static_cast<quint32>(m_known.size()) > pageCount) {
        m_known.resize(static_cast<qsizetype>(pageCount));
    }
    m_fallback = QSizeF(kFallbackWidth, kFallbackHeight);
    for (qsizetype i = m_known.size(); i > 0; --i) {
        if (isSane(m_known.at(i - 1))) {
            m_fallback = m_known.at(i - 1);
            break;
        }
    }
    rebuild();
}

void PageLayout::setArrangement(Spread spread, int rotation) {
    m_spread = spread;
    m_rotation = quarterTurns(rotation);
    rebuild();
}

void PageLayout::rebuild() {
    m_rowCount = rowsFor(m_pageCount, m_spread);
    m_heightBefore.resize(static_cast<qsizetype>(m_rowCount) + 1);
    m_maxWidth = 0.0;
    double leftMax = 0.0;
    double rightMax = 0.0;
    for (quint32 page = 0; page < m_pageCount; ++page) {
        const double width = displaySize(page).width();
        m_maxWidth = std::max(m_maxWidth, width);
        if (m_spread != Spread::One) {
            (slotOf(page) == 0 ? leftMax : rightMax) =
                std::max(slotOf(page) == 0 ? leftMax : rightMax, width);
        }
    }
    if (m_spread == Spread::One) {
        m_halfWidthPoints = m_maxWidth / 2.0;
        m_halfFixed = 0.0;
    } else {
        m_halfWidthPoints = std::max(leftMax, rightMax);
        m_halfFixed = kGap / 2.0;
    }

    double sum = 0.0;
    for (quint32 row = 0; row < m_rowCount; ++row) {
        m_heightBefore[static_cast<qsizetype>(row)] = sum;
        const quint32 first = firstPageOf(row);
        const quint32 count = pagesInRow(row);
        double height = 0.0;
        for (quint32 i = 0; i < count; ++i) {
            height = std::max(height, displaySize(first + i).height());
        }
        sum += height;
    }
    m_heightBefore[static_cast<qsizetype>(m_rowCount)] = sum;
}

QSizeF PageLayout::pageSize(quint32 page) const {
    if (page < static_cast<quint32>(m_known.size())) {
        const QSizeF known = m_known.at(static_cast<qsizetype>(page));
        // A page the engine could not measure arrives as an invalid size.
        if (isSane(known)) {
            return known;
        }
    }
    return m_fallback;
}

QSizeF PageLayout::displaySize(quint32 page) const {
    const QSizeF size = pageSize(page);
    return m_rotation % 2 == 0 ? size : QSizeF(size.height(), size.width());
}

quint32 PageLayout::rowOf(quint32 page) const {
    if (page >= m_pageCount) {
        return m_rowCount;
    }
    switch (m_spread) {
    case Spread::One:
        return page;
    case Spread::Two:
        return page / 2;
    case Spread::TwoCover:
        return page == 0 ? 0 : (page + 1) / 2;
    }
    return page;
}

quint32 PageLayout::firstPageOf(quint32 row) const {
    if (row >= m_rowCount) {
        return m_pageCount;
    }
    switch (m_spread) {
    case Spread::One:
        return row;
    case Spread::Two:
        return 2 * row;
    case Spread::TwoCover:
        return row == 0 ? 0 : 2 * row - 1;
    }
    return row;
}

quint32 PageLayout::pagesInRow(quint32 row) const {
    if (row >= m_rowCount) {
        return 0;
    }
    const quint32 first = firstPageOf(row);
    switch (m_spread) {
    case Spread::One:
        return 1;
    case Spread::Two:
        return std::min<quint32>(2, m_pageCount - first);
    case Spread::TwoCover:
        return row == 0 ? 1 : std::min<quint32>(2, m_pageCount - first);
    }
    return 1;
}

int PageLayout::slotOf(quint32 page) const {
    switch (m_spread) {
    case Spread::One:
        return 0;
    case Spread::Two:
        return static_cast<int>(page % 2);
    case Spread::TwoCover:
        // The cover is alone on the right; the pages after it start on the left.
        return page == 0 ? 1 : (page % 2 == 1 ? 0 : 1);
    }
    return 0;
}

double PageLayout::rowTop(quint32 row, double zoom) const {
    row = std::min(row, m_rowCount);
    return kGap * (static_cast<double>(row) + 1.0) +
           m_heightBefore.at(static_cast<qsizetype>(row)) * zoom;
}

double PageLayout::rowHeight(quint32 row, double zoom) const {
    return rowHeightPoints(row) * zoom;
}

double PageLayout::rowHeightPoints(quint32 row) const {
    if (row >= m_rowCount) {
        return 0.0;
    }
    return m_heightBefore.at(static_cast<qsizetype>(row) + 1) -
           m_heightBefore.at(static_cast<qsizetype>(row));
}

double PageLayout::rowWidthPoints(quint32 row) const {
    const quint32 first = firstPageOf(row);
    double width = 0.0;
    for (quint32 i = 0; i < pagesInRow(row); ++i) {
        width += displaySize(first + i).width();
    }
    return width;
}

double PageLayout::rowFixedWidth(quint32 row) const {
    return pagesInRow(row) == 2 ? kGap : 0.0;
}

quint32 PageLayout::rowAt(double y, double zoom) const {
    if (m_rowCount == 0) {
        return 0;
    }
    // The first row whose top is below `y` ends the search; the row before it holds `y`.
    quint32 low = 0;
    quint32 high = m_rowCount;
    while (low < high) {
        const quint32 mid = low + (high - low) / 2;
        if (rowTop(mid, zoom) <= y) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    return low == 0 ? 0 : low - 1;
}

PageLayout::Range PageLayout::rowsIn(double top, double bottom, double zoom) const {
    if (m_rowCount == 0 || bottom <= top) {
        return {};
    }
    const quint32 first = rowAt(top, zoom);
    quint32 last = rowAt(bottom, zoom);
    // A row whose top is at or below `bottom` is not visible.
    if (last > first && rowTop(last, zoom) >= bottom) {
        --last;
    }
    // The first row may end above `top` (the viewport is in the gap below it).
    const double firstBottom = rowTop(first, zoom) + rowHeight(first, zoom);
    if (firstBottom <= top) {
        if (first >= last) {
            return {};
        }
        return {first + 1, last - first};
    }
    return {first, last - first + 1};
}

QRectF PageLayout::pageRect(quint32 page, double zoom) const {
    const QSizeF size = displaySize(page) * zoom;
    const double top = rowTop(rowOf(page), zoom);
    double left = -size.width() / 2.0;
    if (m_spread != Spread::One) {
        left = slotOf(page) == 0 ? -kGap / 2.0 - size.width() : kGap / 2.0;
    }
    return {left, top, size.width(), size.height()};
}

double PageLayout::pageTop(quint32 page, double zoom) const {
    return rowTop(rowOf(page), zoom);
}

double PageLayout::totalHeight(double zoom) const {
    return rowTop(m_rowCount, zoom);
}

quint32 PageLayout::pageAt(double y, double zoom) const {
    return m_pageCount == 0 ? 0 : firstPageOf(rowAt(y, zoom));
}

PageLayout::Range PageLayout::pagesIn(double top, double bottom, double zoom) const {
    const Range rows = rowsIn(top, bottom, zoom);
    if (rows.count == 0) {
        return {};
    }
    const quint32 first = firstPageOf(rows.first);
    const quint32 lastRow = rows.first + rows.count - 1;
    const quint32 end = firstPageOf(lastRow) + pagesInRow(lastRow);
    return {first, end - first};
}

PageLayout::Anchor PageLayout::anchorAt(double y, double zoom) const {
    const quint32 row = rowAt(y, zoom);
    return {firstPageOf(row), zoom > 0.0 ? (y - rowTop(row, zoom)) / zoom : 0.0};
}

double PageLayout::yOf(const Anchor& anchor, double zoom) const {
    return pageTop(anchor.page, zoom) + anchor.offsetPoints * zoom;
}

QRectF PageLayout::toScreen(const QRectF& points, const QSizeF& file, int rotation, double zoom) {
    const double x = points.x();
    const double y = points.y();
    const double w = points.width();
    const double h = points.height();
    switch (quarterTurns(rotation)) {
    case 1:
        return {(file.height() - (y + h)) * zoom, x * zoom, h * zoom, w * zoom};
    case 2:
        return {(file.width() - (x + w)) * zoom, (file.height() - (y + h)) * zoom, w * zoom,
                h * zoom};
    case 3:
        return {y * zoom, (file.width() - (x + w)) * zoom, h * zoom, w * zoom};
    default:
        return {x * zoom, y * zoom, w * zoom, h * zoom};
    }
}

QRectF PageLayout::toPage(const QRectF& pixels, const QSizeF& file, int rotation, double zoom) {
    const double sx = pixels.x();
    const double sy = pixels.y();
    const double sw = pixels.width();
    const double sh = pixels.height();
    switch (quarterTurns(rotation)) {
    case 1:
        return {sy / zoom, file.height() - (sx + sw) / zoom, sh / zoom, sw / zoom};
    case 2:
        return {file.width() - (sx + sw) / zoom, file.height() - (sy + sh) / zoom, sw / zoom,
                sh / zoom};
    case 3:
        return {file.width() - (sy + sh) / zoom, sx / zoom, sh / zoom, sw / zoom};
    default:
        return {sx / zoom, sy / zoom, sw / zoom, sh / zoom};
    }
}

} // namespace vellora