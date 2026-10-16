#include "text/CaretBrowser.h"

#include <QChar>
#include <algorithm>
#include <cmath>
#include <limits>
#include <utility>

namespace vellora {

namespace {

// Width of the caret in points of the page.
constexpr double kCaretWidth = 1.5;

bool isBreak(char32_t ch) {
    return ch == U'\r' || ch == U'\n';
}

bool isBlank(char32_t ch) {
    return QChar::isSpace(ch);
}

// The text of a page with nothing on it, for a page number that does not exist.
const PageText& emptyPage() {
    static const PageText page = [] {
        PageText empty;
        empty.complete = true;
        return empty;
    }();
    return page;
}

// Keeps a caret off the middle of a CR LF pair, moving it the way it was going.
qsizetype settle(const PageText& text, qsizetype caret, int direction) {
    if (caret > 0 && caret < text.chars.size() && text.chars.at(caret - 1).ch == U'\r' &&
        text.chars.at(caret).ch == U'\n') {
        return direction >= 0 ? caret + 1 : caret - 1;
    }
    return caret;
}

// The index of the boxed character whose edge the caret at `caret` sits on, and whether it is its
// left edge (the caret is before it) or its right edge (the caret is after it).
struct Edge {
    qsizetype index = -1;
    bool left = true;
};

Edge edgeOf(const PageText& text, qsizetype caret) {
    const qsizetype n = text.chars.size();
    if (caret < n && text.chars.at(caret).hasBox()) {
        return {caret, true};
    }
    for (qsizetype i = std::min(caret, n) - 1; i >= 0; --i) {
        if (text.chars.at(i).hasBox()) {
            return {i, false};
        }
    }
    for (qsizetype i = caret; i < n; ++i) {
        if (text.chars.at(i).hasBox()) {
            return {i, true};
        }
    }
    return {};
}

double columnOf(const PageText& text, qsizetype caret) {
    const Edge edge = edgeOf(text, caret);
    if (edge.index < 0) {
        return 0.0;
    }
    const QRectF& rect = text.chars.at(edge.index).rect;
    return edge.left ? rect.left() : rect.right();
}

// The carets of the line that holds character `index`: [first, last], where `last` is the caret at
// the end of its text, before the line break that ends it.
std::pair<qsizetype, qsizetype> lineSpan(const PageText& text, qsizetype index) {
    const qsizetype n = text.chars.size();
    const auto [from, to] = text.lineAround(std::clamp<qsizetype>(index, 0, n - 1));
    qsizetype last = to;
    while (last > static_cast<qsizetype>(from) && isBreak(text.chars.at(last - 1).ch)) {
        --last;
    }
    return {from, last};
}

// The caret in [first, last] whose column is nearest to `column`.
qsizetype nearestCaret(const PageText& text, std::pair<qsizetype, qsizetype> span, double column) {
    qsizetype best = span.first;
    double bestDistance = std::numeric_limits<double>::max();
    for (qsizetype i = span.first; i <= span.second; ++i) {
        const double distance = std::abs(columnOf(text, i) - column);
        if (distance < bestDistance) {
            bestDistance = distance;
            best = i;
        }
    }
    return best;
}

} // namespace

CaretBrowser::CaretBrowser(TextLayer* layer, TextSelection* selection, CanvasController* controller,
                           QObject* parent)
    : QObject(parent), m_layer(layer), m_selection(selection), m_controller(controller) {
    connect(m_layer, &TextLayer::textChanged, this, &CaretBrowser::onTextChanged);
}

void CaretBrowser::reset() {
    const bool wasOn = m_on;
    m_on = false;
    m_position.reset();
    m_anchor.reset();
    m_column.reset();
    m_pending = nullptr;
    if (wasOn) {
        emit changed();
    }
}

const PageText* CaretBrowser::textOf(quint32 page) {
    if (page >= m_controller->pageCount()) {
        return &emptyPage();
    }
    const PageText* text = m_layer->page(page);
    if (text == nullptr || !text->complete) {
        m_layer->ensure(page);
        return nullptr;
    }
    return text;
}

void CaretBrowser::setOn(bool on) {
    if (on == m_on) {
        return;
    }
    m_on = on;
    m_pending = nullptr;
    m_anchor.reset();
    m_column.reset();
    if (!on) {
        m_position.reset();
        emit changed();
        return;
    }
    place();
}

// Where browsing starts: the end of the selection, else the start of the page on screen.
void CaretBrowser::place() {
    if (!m_on) {
        return;
    }
    if (!m_selection->isEmpty()) {
        TextSelection::Position at = m_selection->end();
        const PageText* text = textOf(at.page);
        if (text == nullptr) {
            m_pending = [this] { place(); };
            return;
        }
        at.caret = static_cast<quint32>(std::min<qsizetype>(at.caret, text->chars.size()));
        m_position = at;
    } else {
        m_position = TextSelection::Position{m_controller->currentPage(), 0};
    }
    emit changed();
}

bool CaretBrowser::neighbour(quint32 page, int direction, std::optional<quint32>* found) {
    found->reset();
    qint64 q = static_cast<qint64>(page) + direction;
    while (q >= 0 && q < static_cast<qint64>(m_controller->pageCount())) {
        const PageText* text = textOf(static_cast<quint32>(q));
        if (text == nullptr) {
            return false;
        }
        if (!text->chars.isEmpty()) {
            *found = static_cast<quint32>(q);
            return true;
        }
        q += direction;
    }
    return true;
}

bool CaretBrowser::targetLine(bool down, TextSelection::Position from,
                              TextSelection::Position* to) {
    const PageText* text = textOf(from.page);
    if (text == nullptr) {
        return false;
    }
    *to = from;
    const qsizetype n = text->chars.size();
    if (!m_column) {
        m_column = columnOf(*text, std::min<qsizetype>(from.caret, n));
    }
    if (n > 0) {
        const qsizetype caret = std::min<qsizetype>(from.caret, n);
        const qsizetype index = std::min<qsizetype>(caret, n - 1);
        const auto full = text->lineAround(index);
        if (down && static_cast<qsizetype>(full.second) < n) {
            const auto span = lineSpan(*text, full.second);
            to->caret = static_cast<quint32>(nearestCaret(*text, span, *m_column));
            return true;
        }
        if (!down && full.first > 0) {
            const auto span = lineSpan(*text, static_cast<qsizetype>(full.first) - 1);
            to->caret = static_cast<quint32>(nearestCaret(*text, span, *m_column));
            return true;
        }
    }
    // Past the first or last line of the page: the nearest line of the next page with text.
    std::optional<quint32> other;
    if (!neighbour(from.page, down ? 1 : -1, &other)) {
        return false;
    }
    if (!other) {
        return true; // the top or bottom of the document: stay
    }
    const PageText* page = textOf(*other);
    if (page == nullptr) {
        return false;
    }
    const auto span = down ? lineSpan(*page, 0) : lineSpan(*page, page->chars.size() - 1);
    to->page = *other;
    to->caret = static_cast<quint32>(nearestCaret(*page, span, *m_column));
    return true;
}

bool CaretBrowser::target(Move move, TextSelection::Position from, TextSelection::Position* to) {
    if (move == Move::Up || move == Move::Down) {
        return targetLine(move == Move::Down, from, to);
    }
    if (move == Move::DocumentStart) {
        *to = {0, 0};
        return true;
    }
    if (move == Move::DocumentEnd) {
        const quint32 count = m_controller->pageCount();
        const quint32 last = count == 0 ? 0 : count - 1;
        const PageText* text = textOf(last);
        if (text == nullptr) {
            return false;
        }
        *to = {last, static_cast<quint32>(text->chars.size())};
        return true;
    }
    const PageText* text = textOf(from.page);
    if (text == nullptr) {
        return false;
    }
    *to = from;
    const qsizetype n = text->chars.size();
    const qsizetype caret = std::min<qsizetype>(from.caret, n);
    // A move that runs off the page goes to the end of the previous one, or the start of the next.
    const auto cross = [&](int direction) {
        std::optional<quint32> other;
        if (!neighbour(from.page, direction, &other)) {
            return false;
        }
        if (other) {
            const PageText* page = textOf(*other);
            if (page == nullptr) {
                return false;
            }
            to->page = *other;
            to->caret = direction > 0 ? 0U : static_cast<quint32>(page->chars.size());
        }
        return true;
    };
    switch (move) {
    case Move::Left:
        if (caret > 0) {
            to->caret = static_cast<quint32>(settle(*text, caret - 1, -1));
            return true;
        }
        return cross(-1);
    case Move::Right:
        if (caret < n) {
            to->caret = static_cast<quint32>(settle(*text, caret + 1, 1));
            return true;
        }
        return cross(1);
    case Move::WordRight: {
        qsizetype i = caret;
        while (i < n && !isBlank(text->chars.at(i).ch)) {
            ++i;
        }
        while (i < n && isBlank(text->chars.at(i).ch)) {
            ++i;
        }
        if (i == caret) {
            return cross(1);
        }
        to->caret = static_cast<quint32>(i);
        return true;
    }
    case Move::WordLeft: {
        qsizetype i = caret;
        while (i > 0 && isBlank(text->chars.at(i - 1).ch)) {
            --i;
        }
        while (i > 0 && !isBlank(text->chars.at(i - 1).ch)) {
            --i;
        }
        if (i == caret) {
            return cross(-1);
        }
        to->caret = static_cast<quint32>(i);
        return true;
    }
    case Move::LineStart:
    case Move::LineEnd: {
        if (n == 0) {
            return true;
        }
        const auto span = lineSpan(*text, std::min<qsizetype>(caret, n - 1));
        to->caret = static_cast<quint32>(move == Move::LineStart ? span.first : span.second);
        return true;
    }
    default:
        return true;
    }
}

bool CaretBrowser::move(Move move, bool extend) {
    if (!m_on) {
        return false;
    }
    m_pending = nullptr;
    if (!m_position) {
        return true; // still being placed
    }
    const bool vertical = move == Move::Up || move == Move::Down;
    if (!vertical) {
        m_column.reset();
    }
    if (extend && !m_anchor) {
        m_anchor = m_position;
    }
    TextSelection::Position to;
    if (!target(move, *m_position, &to)) {
        // A page it needs is on its way; the move is made when it is here.
        m_pending = [this, move, extend] { this->move(move, extend); };
        return true;
    }
    arrive(to, extend);
    return true;
}

void CaretBrowser::arrive(TextSelection::Position at, bool extend) {
    m_position = at;
    if (extend && m_anchor) {
        m_selection->setRange(*m_anchor, at);
    } else {
        m_anchor.reset();
        m_selection->clear();
    }
    emit changed();
    if (const auto c = caret(); c && !c->rect.isEmpty()) {
        m_controller->revealRect(c->page, c->rect);
    }
}

void CaretBrowser::placeAt(quint32 page, QPointF point) {
    if (!m_on) {
        return;
    }
    const PageText* text = textOf(page);
    if (text == nullptr) {
        m_pending = [this, page, point] { placeAt(page, point); };
        return;
    }
    m_pending = nullptr;
    m_anchor.reset();
    m_column.reset();
    m_position = TextSelection::Position{page, text->caretAt(point)};
    emit changed();
}

std::optional<CaretBrowser::Caret> CaretBrowser::caret() const {
    if (!m_on || !m_position) {
        return std::nullopt;
    }
    Caret out;
    out.page = m_position->page;
    const PageText* text = m_layer->page(m_position->page);
    if (text == nullptr || text->chars.isEmpty()) {
        return out;
    }
    const qsizetype caret = std::min<qsizetype>(m_position->caret, text->chars.size());
    const Edge edge = edgeOf(*text, caret);
    if (edge.index < 0) {
        return out;
    }
    const QRectF& box = text->chars.at(edge.index).rect;
    const double x = edge.left ? box.left() : box.right();
    out.rect = QRectF(x - kCaretWidth / 2.0, box.top(), kCaretWidth, box.height());
    return out;
}

void CaretBrowser::onTextChanged(quint32) {
    if (m_pending) {
        const auto retry = std::exchange(m_pending, nullptr);
        retry();
    }
    // The caret may be on a page whose boxes have only now arrived.
    if (m_on && m_position) {
        emit changed();
    }
}

} // namespace vellora
