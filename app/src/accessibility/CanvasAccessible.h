// What a screen reader sees of the canvas. The canvas draws pages with the GPU, so there are no
// widgets to describe; the accessibility tree is built from the same data the canvas shows:
//
//   document (the canvas view)
//     page 1 .. page N       text of the page, in reading order, through the text interface
//       link ..              focusable, can be followed
//
// The interfaces read the layers live (they keep only a page number and an index), so they stay
// right when the view moves, the document is reloaded or the text arrives. A page's text is asked
// for from the engine when a reader first looks at it, and a text-insert event announces what
// arrives.
#pragma once

#include <QtGlobal>

class QAccessibleInterface;

namespace vellora {

class CanvasView;

// Teaches Qt's accessibility layer to describe `CanvasView`. Idempotent; the view's constructor
// calls it.
void installCanvasAccessibility();

// The accessible interface of link `index` of `page`, or null if accessibility is not available
// for the view or there is no such link. Owned by the accessibility layer.
QAccessibleInterface* canvasLinkInterface(CanvasView* view, quint32 page, int index);

} // namespace vellora
