// Where an outline item (and, later, a link) takes the reader, and one outline item, in the Qt
// shell's terms. The engine's answers are turned into these by EngineSession.
#pragma once

#include "rust/cxx.h"
#include "vellora-engine-client/src/bridge.rs.h"

#include <QMetaType>
#include <QString>
#include <cmath>
#include <limits>

namespace vellora {

// A page and how to show it (ISO 32000-2 Table 151). Coordinates are in default user space units
// of the page; a value the destination leaves as it is, or does not have, is NaN.
struct Destination {
    static constexpr double kNone = std::numeric_limits<double>::quiet_NaN();

    quint32 page = 0;
    FitKind fit = FitKind::None;
    double left = kNone;
    double top = kNone;
    double right = kNone;
    double bottom = kNone;
    double zoom = kNone; // 1.0 is 100%

    bool valid() const { return fit != FitKind::None; }
    static bool has(double value) { return std::isfinite(value); }
};

struct OutlineItem {
    quint32 id = 0; // names the item in later requests; meaningful for this document only
    QString title;  // plain text
    Destination destination;
    bool hasChildren = false;
    bool open = false; // the document asks for the children to be shown when it opens
    bool bold = false;
    bool italic = false;
};

} // namespace vellora

Q_DECLARE_METATYPE(vellora::Destination)
Q_DECLARE_METATYPE(vellora::OutlineItem)
