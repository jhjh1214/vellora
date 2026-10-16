#include "print/PrintLayout.h"

#include <algorithm>

namespace vellora {

namespace {

// The engine renders at most this many pixels per point (`MAX_TILE_SCALE` of the protocol).
constexpr double kMaxRenderScale = 64.0;

} // namespace

Placement placePage(QSizeF page, QSizeF printable, double dpi, const PrintOptions& options) {
    Placement placement;
    const double devicePerPoint = dpi / 72.0;
    placement.rotated = options.autoRotate && ((page.width() > page.height()) !=
                                               (printable.width() > printable.height()));
    const QSizeF shown = placement.rotated ? QSizeF(page.height(), page.width()) : page;

    const double fit = std::min(printable.width() / (shown.width() * devicePerPoint),
                                printable.height() / (shown.height() * devicePerPoint));
    double factor = 1.0;
    switch (options.scale) {
    case PrintScale::FitToPage:
        factor = fit;
        break;
    case PrintScale::ActualSize:
        factor = 1.0;
        break;
    case PrintScale::ShrinkOversized:
        factor = std::min(1.0, fit);
        break;
    }
    placement.scale = devicePerPoint * factor;
    placement.renderScale = placement.scale;
    if (options.maxDpi > 0) {
        placement.renderScale = std::min(placement.renderScale, options.maxDpi / 72.0 * factor);
    }
    placement.renderScale = std::min(placement.renderScale, kMaxRenderScale);
    placement.size = QSizeF(shown.width() * placement.scale, shown.height() * placement.scale);
    placement.origin = options.center
                           ? QPointF((printable.width() - placement.size.width()) / 2.0,
                                     (printable.height() - placement.size.height()) / 2.0)
                           : QPointF(0.0, 0.0);
    return placement;
}

QList<quint32> filterParity(const QList<quint32>& pages, PageParity parity) {
    if (parity == PageParity::All) {
        return pages;
    }
    QList<quint32> kept;
    for (const quint32 page : pages) {
        const bool odd = (page % 2U) == 0U; // page 0 is page number 1
        if ((parity == PageParity::Odd) == odd) {
            kept.append(page);
        }
    }
    return kept;
}

QList<quint32> sheetOrder(const QList<quint32>& pages, int copies, bool collate,
                          bool lastPageFirst) {
    QList<quint32> ordered = pages;
    if (lastPageFirst) {
        std::reverse(ordered.begin(), ordered.end());
    }
    const int count = std::max(copies, 1);
    QList<quint32> sheets;
    sheets.reserve(ordered.size() * count);
    if (collate) {
        for (int copy = 0; copy < count; ++copy) {
            sheets.append(ordered);
        }
    } else {
        for (const quint32 page : ordered) {
            for (int copy = 0; copy < count; ++copy) {
                sheets.append(page);
            }
        }
    }
    return sheets;
}

} // namespace vellora
