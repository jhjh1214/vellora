// The selected text: from one caret to another, across any number of pages. The model only holds
// the two ends (and what unit the reader selected by); it asks the text layer for the geometry
// and the characters when they are needed.
//
// A drag selects by character; a double click selects a word and a drag from it goes on by whole
// words; a triple click selects a line and goes on by whole lines. Shift+click moves the end that
// the reader did not start from.
//
// Copying reads the text of every page in the selection from the engine, one after another, so
// the whole document can be copied without holding it: `copyText` is asynchronous.
#pragma once

#include "text/TextLayer.h"

#include <QList>
#include <QObject>
#include <QPointF>
#include <QRectF>
#include <QString>
#include <functional>
#include <limits>

namespace vellora {

class TextSelection : public QObject {
    Q_OBJECT

public:
    enum class Unit { Character, Word, Line };

    // A place between two characters of a page.
    struct Position {
        quint32 page = 0;
        quint32 caret = 0;
        friend auto operator<=>(const Position&, const Position&) = default;
    };
    // To the end of a page, whatever its length (the pages in the middle of a selection).
    static constexpr quint32 kToEnd = std::numeric_limits<quint32>::max();

    explicit TextSelection(TextLayer* layer, QObject* parent = nullptr);

    bool isEmpty() const { return !m_active || start() == end(); }
    // The two ends in document order; meaningful when not empty.
    Position start() const;
    Position end() const;
    void clear();

    // Starts a selection at `point` (points of the page as shown) of `page`, by `unit`. With
    // `extend` and a selection, it moves the end the reader did not start from instead. False when
    // the page's text has not arrived yet (nothing changed).
    bool begin(quint32 page, QPointF point, Unit unit, bool extend = false);
    // Moves the end being dragged to `point` of `page`.
    void extendTo(quint32 page, QPointF point);
    // The text of one page (`kToEnd` carets), or of the whole document.
    void selectPage(quint32 page);
    void selectAll(quint32 pageCount);

    // The part of `page` that is selected, as carets (`to` may be `kToEnd`); false if none of it.
    bool rangeOn(quint32 page, quint32* from, quint32* to) const;
    // Rectangles to highlight on `page` (points of the page as shown), one per line. Needs the
    // page's text; empty if it has not arrived.
    QList<QRectF> rectsOn(quint32 page) const;

    // The selected text, read page by page from the engine. `done` is called once with the text,
    // or never if the selection changes first or the text cannot be read (then `failed`).
    void copyText(std::function<void(const QString&)> done);
    bool copying() const { return m_copyPage != kNoPage; }

signals:
    void changed();
    // The text of a page in the selection could not be had (the engine is down).
    void copyFailed();

private slots:
    void onTextChanged(quint32 page);

private:
    static constexpr quint32 kNoPage = std::numeric_limits<quint32>::max();

    // The carets around `point` for `unit`: the range the unit stands for.
    bool unitAt(quint32 page, QPointF point, Unit unit, Position* lo, Position* hi) const;
    void stepCopy();
    void cancelCopy();

    TextLayer* m_layer;
    bool m_active = false;
    Unit m_unit = Unit::Character;
    // The range the reader started with, and the range under the pointer now.
    Position m_anchorLo, m_anchorHi;
    Position m_focusLo, m_focusHi;

    // Copying: the page being read, the last one, what has been read.
    quint32 m_copyPage = kNoPage;
    quint32 m_copyLast = 0;
    QString m_copyText;
    std::function<void(const QString&)> m_copyDone;
};

} // namespace vellora
