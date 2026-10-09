#include "text/TextSelection.h"

#include <algorithm>

namespace vellora {

TextSelection::TextSelection(TextLayer* layer, QObject* parent) : QObject(parent), m_layer(layer) {
    connect(m_layer, &TextLayer::textChanged, this, &TextSelection::onTextChanged);
}

TextSelection::Position TextSelection::start() const {
    return std::min(m_anchorLo, m_focusLo);
}

TextSelection::Position TextSelection::end() const {
    return std::max(m_anchorHi, m_focusHi);
}

void TextSelection::clear() {
    cancelCopy();
    if (m_active) {
        m_active = false;
        emit changed();
    }
}

bool TextSelection::unitAt(quint32 page, QPointF point, Unit unit, Position* lo,
                           Position* hi) const {
    const PageText* text = m_layer->page(page);
    if (text == nullptr || !text->complete) {
        return false;
    }
    if (unit == Unit::Character) {
        const quint32 caret = text->caretAt(point);
        *lo = *hi = {page, caret};
        return true;
    }
    // A word or a line needs a character under (or beside) the pointer.
    qsizetype index = text->charAt(point);
    if (index < 0) {
        const quint32 caret = text->caretAt(point);
        index = static_cast<qsizetype>(
            std::min<quint32>(caret, static_cast<quint32>(text->chars.size())));
        if (index >= text->chars.size()) {
            index = text->chars.size() - 1;
        }
    }
    if (index < 0) {
        return false;
    }
    const auto [from, to] = unit == Unit::Word ? text->wordAround(index) : text->lineAround(index);
    *lo = {page, from};
    *hi = {page, to};
    return true;
}

bool TextSelection::begin(quint32 page, QPointF point, Unit unit, bool extend) {
    Position lo;
    Position hi;
    if (!unitAt(page, point, unit, &lo, &hi)) {
        return false;
    }
    cancelCopy();
    if (extend && m_active) {
        // The end the reader did not start from moves; the starting unit stays.
        m_focusLo = lo;
        m_focusHi = hi;
    } else {
        m_unit = unit;
        m_anchorLo = m_focusLo = lo;
        m_anchorHi = m_focusHi = hi;
        m_active = true;
    }
    emit changed();
    return true;
}

void TextSelection::extendTo(quint32 page, QPointF point) {
    if (!m_active) {
        return;
    }
    Position lo;
    Position hi;
    if (!unitAt(page, point, m_unit, &lo, &hi)) {
        return;
    }
    if (lo != m_focusLo || hi != m_focusHi) {
        cancelCopy();
        m_focusLo = lo;
        m_focusHi = hi;
        emit changed();
    }
}

void TextSelection::selectPage(quint32 page) {
    cancelCopy();
    m_active = true;
    m_unit = Unit::Character;
    m_anchorLo = m_focusLo = {page, 0};
    m_anchorHi = m_focusHi = {page, kToEnd};
    emit changed();
}

void TextSelection::selectAll(quint32 pageCount) {
    if (pageCount == 0) {
        clear();
        return;
    }
    cancelCopy();
    m_active = true;
    m_unit = Unit::Character;
    m_anchorLo = m_focusLo = {0, 0};
    m_anchorHi = m_focusHi = {pageCount - 1, kToEnd};
    emit changed();
}

bool TextSelection::rangeOn(quint32 page, quint32* from, quint32* to) const {
    if (isEmpty()) {
        return false;
    }
    const Position s = start();
    const Position e = end();
    if (page < s.page || page > e.page) {
        return false;
    }
    *from = page == s.page ? s.caret : 0;
    *to = page == e.page ? e.caret : kToEnd;
    return *from < *to;
}

QList<QRectF> TextSelection::rectsOn(quint32 page) const {
    quint32 from = 0;
    quint32 to = 0;
    const PageText* text = m_layer->page(page);
    if (text == nullptr || !rangeOn(page, &from, &to)) {
        return {};
    }
    return text->rects(from, to);
}

// ---- copying ----

void TextSelection::copyText(std::function<void(const QString&)> done) {
    cancelCopy();
    if (isEmpty()) {
        return;
    }
    m_copyDone = std::move(done);
    m_copyText.clear();
    m_copyPage = start().page;
    m_copyLast = end().page;
    stepCopy();
}

void TextSelection::cancelCopy() {
    m_copyPage = kNoPage;
    m_copyDone = nullptr;
    m_copyText.clear();
}

void TextSelection::stepCopy() {
    // Reads as many pages as are at hand; asks for the next one that is not.
    while (m_copyPage != kNoPage) {
        const PageText* text = m_layer->page(m_copyPage);
        if (text == nullptr || !text->complete) {
            m_layer->ensure(m_copyPage);
            if (m_layer->page(m_copyPage) == nullptr) {
                cancelCopy(); // nothing asked and nothing coming: the engine is down
                emit copyFailed();
            }
            return; // otherwise `onTextChanged` continues
        }
        quint32 from = 0;
        quint32 to = 0;
        if (rangeOn(m_copyPage, &from, &to)) {
            if (!m_copyText.isEmpty()) {
                m_copyText.append(QLatin1Char('\n')); // between pages
            }
            m_copyText.append(text->text(from, to));
        }
        if (m_copyPage >= m_copyLast) {
            const QString result = std::move(m_copyText);
            const auto done = std::move(m_copyDone);
            cancelCopy();
            if (done) {
                done(result);
            }
            return;
        }
        ++m_copyPage;
    }
}

void TextSelection::onTextChanged(quint32 page) {
    if (m_copyPage != kNoPage && page == m_copyPage) {
        stepCopy();
    }
    // The highlight of a page changes when its text arrives.
    if (!isEmpty()) {
        emit changed();
    }
}

} // namespace vellora
