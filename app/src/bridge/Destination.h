// Where an outline item (and, later, a link) takes the reader, and one outline item, in the Qt
// shell's terms. The engine's answers are turned into these by EngineSession.
#pragma once

#include "rust/cxx.h"
#include "vellora-engine-client/src/bridge.rs.h"

#include <QMetaType>
#include <QRectF>
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

// A link on a page: where it is and what it does. The engine only describes links; what to do is
// decided by the shell (internal jumps are followed, addresses are confirmed, the rest is never
// run).
struct Link {
    QRectF rect; // points of the page as shown, origin at the top left
    LinkKind kind = LinkKind::Unresolved;
    NamedKind named = NamedKind::None; // for `Named`
    QString text;                      // `Uri`: the address; `Inert`: the name of the action
    Destination destination;           // for `GoTo`
};

// One character of a page's text, in the order PDFium extracts it.
struct TextChar {
    static constexpr quint8 kGenerated = 1; // inserted by PDFium: a space or a break, with no box
    static constexpr quint8 kHyphen = 2;    // a hyphen that breaks a word at the end of a line

    char32_t ch = 0;
    QRectF rect;    // points of the page as shown, origin at the top left; empty if generated
    float size = 0; // font size in points
    quint8 flags = 0;
    quint32 word = 0; // index of the word on the page
    quint32 line = 0; // index of the line on the page

    bool generated() const { return (flags & kGenerated) != 0; }
    // Whether the pointer can be on it: it has a box.
    bool hasBox() const { return !generated() && !rect.isEmpty(); }
};

} // namespace vellora

Q_DECLARE_METATYPE(vellora::TextChar)
Q_DECLARE_METATYPE(vellora::Link)
Q_DECLARE_METATYPE(vellora::Destination)
Q_DECLARE_METATYPE(vellora::OutlineItem)
