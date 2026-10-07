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

    m_heightBefore.resize(static_cast<qsizetype>(pageCount) + 1);
    m_maxWidth = 0.0;
    double sum = 0.0;
    for (quint32 i = 0; i < pageCount; ++i) {
        m_heightBefore[static_cast<qsizetype>(i)] = sum;
        const QSizeF size = pageSize(i);
        sum += size.height();
        m_maxWidth = std::max(m_maxWidth, size.width());
    }
    m_heightBefore[static_cast<qsizetype>(pageCount)] = sum;
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

double PageLayout::pageTop(quint32 page, double zoom) const {
    page = std::min(page, m_pageCount);
    return kGap * (static_cast<double>(page) + 1.0) +
           m_heightBefore.at(static_cast<qsizetype>(page)) * zoom;
}

double PageLayout::totalHeight(double zoom) const {
    return kGap * (static_cast<double>(m_pageCount) + 1.0) + m_heightBefore.last() * zoom;
}

quint32 PageLayout::pageAt(double y, double zoom) const {
    if (m_pageCount == 0) {
        return 0;
    }
    // The first page whose top is below `y` ends the search; the page before it holds `y`.
    quint32 low = 0;
    quint32 high = m_pageCount;
    while (low < high) {
        const quint32 mid = low + (high - low) / 2;
        if (pageTop(mid, zoom) <= y) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    return low == 0 ? 0 : low - 1;
}

PageLayout::Range PageLayout::pagesIn(double top, double bottom, double zoom) const {
    if (m_pageCount == 0 || bottom <= top) {
        return {};
    }
    const quint32 first = pageAt(top, zoom);
    quint32 last = pageAt(bottom, zoom);
    // A page whose top is at or below `bottom` is not visible.
    if (last > first && pageTop(last, zoom) >= bottom) {
        --last;
    }
    // The first page may end above `top` (the viewport is in the gap below it).
    const double firstBottom = pageTop(first, zoom) + pageSize(first).height() * zoom;
    if (firstBottom <= top) {
        if (first >= last) {
            return {};
        }
        return {first + 1, last - first};
    }
    return {first, last - first + 1};
}

PageLayout::Anchor PageLayout::anchorAt(double y, double zoom) const {
    const quint32 page = pageAt(y, zoom);
    return {page, zoom > 0.0 ? (y - pageTop(page, zoom)) / zoom : 0.0};
}

double PageLayout::yOf(const Anchor& anchor, double zoom) const {
    return pageTop(anchor.page, zoom) + anchor.offsetPoints * zoom;
}

} // namespace vellora
