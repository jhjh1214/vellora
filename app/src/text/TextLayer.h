// The text of the pages on screen. The engine is asked for the characters of each page that comes
// into view (windows of 8,192 until the page is complete) and the answers are kept for the pages
// that stay near it. A page's text knows where its lines are, so the pointer can be turned into a
// position between two characters and into the word or line around one.
//
// A position is a *caret*: a number from 0 to the page's character count, the gap before the
// character with that index. A selection is the characters between two carets. Left to right,
// top to bottom text is exact; right-to-left and vertical text keep PDFium's order and boxes, so
// a caret there is placed by the left or right half of a box and the highlight may be imperfect
// (the known limits are in docs/user/text-selection.md).
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"

#include <QHash>
#include <QList>
#include <QObject>
#include <QPointF>
#include <QRectF>
#include <QVector>
#include <utility>

namespace vellora {

// The characters of one page and what is derived from them.
class PageText {
public:
    QList<TextChar> chars;
    quint32 total = 0; // how many the page has in all, as the engine said
    bool complete = false;

    // Call after the last window arrived (or when a page's text cannot be had): builds the lines.
    void finish();

    // A run of boxed characters with the same line number. Text on a page turned a quarter runs
    // up or down the page as shown, and right-to-left text runs against the x axis: a line knows
    // along which axis and which way its characters follow each other.
    struct Line {
        qsizetype first = 0;   // index of its first boxed character
        qsizetype last = 0;    // index of its last boxed character
        QRectF rect;           // everything its boxed characters cover
        bool vertical = false; // the characters follow each other along y
        bool reversed = false; // ... towards smaller coordinates
    };
    const QList<Line>& lines() const { return m_lines; }

    // The caret nearest to `point` (points of the page as shown): between the characters of the
    // line the point is on, or on the nearest line. 0 for a page without boxed characters.
    quint32 caretAt(QPointF point) const;
    // The index of the character whose box contains `point`, or -1.
    qsizetype charAt(QPointF point) const;
    // Whether the pointer is over text: a box contains it.
    bool isOverText(QPointF point) const { return charAt(point) >= 0; }

    // The carets that enclose the word, or the line, around the character at `index`.
    std::pair<quint32, quint32> wordAround(qsizetype index) const;
    std::pair<quint32, quint32> lineAround(qsizetype index) const;

    // Rectangles for the characters from caret `from` up to caret `to` (exclusive), one per line.
    QList<QRectF> rects(quint32 from, quint32 to) const;
    // Their text, `\r\n` and `\r` as `\n`.
    QString text(quint32 from, quint32 to) const;

private:
    // The line nearest to `point` (0 distance inside its rectangle), or -1 if there is none.
    qsizetype lineNear(QPointF point) const;

    QList<Line> m_lines;
};

class TextLayer : public QObject {
    Q_OBJECT

public:
    static constexpr quint32 kPerRequest = 8192;
    // Pages whose text is kept; the ones furthest from the view go first.
    static constexpr int kMaxCachedPages = 32;

    // Neither is owned; both must outlive the layer.
    TextLayer(EngineSession* session, CanvasController* controller, QObject* parent = nullptr);

    // Forgets the previous document's text. Call before the session opens another file.
    void reset();

    // The text of `page` read so far, or null if none was asked for. `isComplete` says whether it
    // is all there.
    const PageText* page(quint32 pageIndex) const;
    bool isComplete(quint32 pageIndex) const;
    // Asks for the text of `page` if it is not kept (the pages in view are asked for by
    // themselves). `textChanged` follows every window.
    void ensure(quint32 pageIndex);
    int requestsInFlight() const { return static_cast<int>(m_requests.size()); }

signals:
    // More of the text of `page` has arrived, or all of it (`isComplete`).
    void textChanged(quint32 page);

private slots:
    void onOpened();
    void onText(quint64 request, quint32 page, quint32 skip, quint32 total,
                const QList<vellora::TextChar>& chars);
    void onRequestFailed(quint64 request);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void wantVisible();

private:
    struct Entry {
        PageText text;
        quint64 request = 0;
    };

    void viewMoved();
    void askFor(quint32 pageIndex);
    void evict(const QVector<PageDraw>& visible);

    EngineSession* m_session;
    CanvasController* m_controller;
    QHash<quint32, Entry> m_pages;
    QHash<quint64, quint32> m_requests;
    bool m_queued = false;
};

} // namespace vellora
