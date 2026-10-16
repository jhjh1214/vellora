// Caret browsing: a text cursor that moves through the text of the document with the keyboard, so
// that it can be read and selected without a mouse (F7 turns it on and off). It moves by
// character, word and line, across pages, and with Shift it selects from where it started.
//
// The caret is a position between two characters, as in `TextSelection`. Moving needs the text of
// the page, which arrives from the engine; a move that has to wait for a page is carried out when
// the page is there. Left and right follow the order of the characters (reading order), up and
// down follow the lines as the page shows them, keeping the column the caret was in.
#pragma once

#include "canvas/CanvasController.h"
#include "text/TextLayer.h"
#include "text/TextSelection.h"

#include <QObject>
#include <QRectF>
#include <functional>
#include <optional>

namespace vellora {

class CaretBrowser : public QObject {
    Q_OBJECT

public:
    enum class Move {
        Left,
        Right,
        WordLeft,
        WordRight,
        Up,
        Down,
        LineStart,
        LineEnd,
        DocumentStart,
        DocumentEnd,
    };

    // None is owned; all must outlive the browser.
    CaretBrowser(TextLayer* layer, TextSelection* selection, CanvasController* controller,
                 QObject* parent = nullptr);

    bool isOn() const { return m_on; }
    // Turning it on puts the caret at the end of the selection if there is one, else at the start
    // of the page on screen. Turning it off leaves the selection as it is.
    void setOn(bool on);

    // Where the caret is (once the text it needs has arrived).
    std::optional<TextSelection::Position> position() const { return m_position; }

    // Moves the caret; with `extend` the selection grows from where the caret was when the move
    // began (the anchor), else the selection is cleared. Returns false if browsing is off.
    bool move(Move move, bool extend);
    // Puts the caret at `point` of `page` (a click while browsing).
    void placeAt(quint32 page, QPointF point);

    // The caret as a thin rectangle of its page (points of the page as shown), for painting and
    // for scrolling to. Empty rectangle if there is none or the page has no boxes.
    struct Caret {
        quint32 page = 0;
        QRectF rect;
    };
    std::optional<Caret> caret() const;

    // Forgets everything and turns browsing off (a new document).
    void reset();

signals:
    // Browsing was turned on or off, or the caret moved.
    void changed();

private slots:
    void onTextChanged(quint32 page);

private:
    // The text of `page` if it is all here; else asks for it and returns null.
    const PageText* textOf(quint32 page);
    // Computes where `move` leads from `from`; false if a page it needs has not arrived (it has
    // been asked for).
    bool target(Move move, TextSelection::Position from, TextSelection::Position* to);
    bool targetLine(bool down, TextSelection::Position from, TextSelection::Position* to);
    // The nearest page in `direction` (+1 or -1) from `page` that has text and is all here; false
    // if one has not arrived yet; `found` stays empty when there is none.
    bool neighbour(quint32 page, int direction, std::optional<quint32>* found);
    void arrive(TextSelection::Position at, bool extend);
    void place();

    TextLayer* m_layer;
    TextSelection* m_selection;
    CanvasController* m_controller;
    bool m_on = false;
    std::optional<TextSelection::Position> m_position;
    // The end of the selection that does not move while it grows.
    std::optional<TextSelection::Position> m_anchor;
    // A move waiting for a page: tried again whenever text arrives.
    std::function<void()> m_pending;
    // The column (points, page as shown) vertical moves aim for, kept across a run of them.
    std::optional<double> m_column;
    bool m_lastWasVertical = false;
};

} // namespace vellora
