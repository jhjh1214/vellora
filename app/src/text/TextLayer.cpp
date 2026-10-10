#include "text/TextLayer.h"

#include <QMetaObject>
#include <algorithm>
#include <cmath>
#include <limits>

namespace vellora {

// ---- PageText ----

void PageText::finish() {
    complete = true;
    m_lines.clear();
    // Lines are runs of boxed characters with the same line number.
    for (qsizetype i = 0; i < chars.size(); ++i) {
        const TextChar& c = chars.at(i);
        if (!c.hasBox()) {
            continue;
        }
        if (!m_lines.isEmpty() && chars.at(m_lines.last().last).line == c.line) {
            Line& line = m_lines.last();
            line.last = i;
            line.rect = line.rect.united(c.rect);
        } else {
            m_lines.append({i, i, c.rect, false, false});
        }
    }
    // Which way each line runs: from where its first character is to where its last one is.
    for (Line& line : m_lines) {
        const QPointF run = chars.at(line.last).rect.center() - chars.at(line.first).rect.center();
        line.vertical = std::abs(run.y()) > std::abs(run.x());
        line.reversed = line.vertical ? run.y() < 0.0 : run.x() < 0.0;
    }
}

qsizetype PageText::lineNear(QPointF point) const {
    // The line whose rectangle is nearest *across* its direction (a point far below the text is on
    // the last line however long the first is), and among those the nearest along it.
    qsizetype best = -1;
    double bestAcross = std::numeric_limits<double>::infinity();
    double bestAlong = std::numeric_limits<double>::infinity();
    const auto gap = [](double value, double low, double high) {
        return value < low ? low - value : (value > high ? value - high : 0.0);
    };
    for (qsizetype i = 0; i < m_lines.size(); ++i) {
        const Line& line = m_lines.at(i);
        const QRectF& r = line.rect;
        const double dx = gap(point.x(), r.left(), r.right());
        const double dy = gap(point.y(), r.top(), r.bottom());
        const double across = line.vertical ? dx : dy;
        const double along = line.vertical ? dy : dx;
        if (across < bestAcross || (across == bestAcross && along < bestAlong)) {
            best = i;
            bestAcross = across;
            bestAlong = along;
        }
    }
    return best;
}

qsizetype PageText::charAt(QPointF point) const {
    for (const Line& line : m_lines) {
        if (!line.rect.contains(point)) {
            continue;
        }
        for (qsizetype i = line.first; i <= line.last; ++i) {
            const TextChar& c = chars.at(i);
            if (c.hasBox() && c.rect.contains(point)) {
                return i;
            }
        }
    }
    return -1;
}

quint32 PageText::caretAt(QPointF point) const {
    const qsizetype lineIndex = lineNear(point);
    if (lineIndex < 0) {
        return 0;
    }
    const Line& line = m_lines.at(lineIndex);
    // The first boxed character whose middle is ahead of the point (along the line) puts the
    // caret before it.
    const double along = line.vertical ? point.y() : point.x();
    qsizetype last = line.first;
    for (qsizetype i = line.first; i <= line.last; ++i) {
        const TextChar& c = chars.at(i);
        if (!c.hasBox()) {
            continue;
        }
        last = i;
        const double middle = line.vertical ? c.rect.center().y() : c.rect.center().x();
        if (line.reversed ? along > middle : along < middle) {
            return static_cast<quint32>(i);
        }
    }
    return static_cast<quint32>(last) + 1;
}

std::pair<quint32, quint32> PageText::wordAround(qsizetype index) const {
    if (index < 0 || index >= chars.size()) {
        return {0, 0};
    }
    const quint32 word = chars.at(index).word;
    qsizetype from = index;
    qsizetype to = index;
    while (from > 0 && chars.at(from - 1).word == word) {
        --from;
    }
    while (to + 1 < chars.size() && chars.at(to + 1).word == word) {
        ++to;
    }
    // White space belongs to the word before it; selecting a word does not take what follows it.
    // (On white space itself, the white space is what is selected.)
    const auto blank = [this](qsizetype i) {
        const char32_t ch = chars.at(i).ch;
        return ch == U' ' || ch == U'\t' || ch == U'\r' || ch == U'\n' || QChar::isSpace(ch);
    };
    if (blank(index)) {
        while (from < index && !blank(from)) {
            ++from; // skip the letters before the white space
        }
    } else {
        while (to > index && blank(to)) {
            --to;
        }
    }
    return {static_cast<quint32>(from), static_cast<quint32>(to) + 1};
}

std::pair<quint32, quint32> PageText::lineAround(qsizetype index) const {
    if (index < 0 || index >= chars.size()) {
        return {0, 0};
    }
    const quint32 line = chars.at(index).line;
    qsizetype from = index;
    qsizetype to = index;
    while (from > 0 && chars.at(from - 1).line == line) {
        --from;
    }
    while (to + 1 < chars.size() && chars.at(to + 1).line == line) {
        ++to;
    }
    return {static_cast<quint32>(from), static_cast<quint32>(to) + 1};
}

QList<QRectF> PageText::rects(quint32 from, quint32 to) const {
    QList<QRectF> out;
    const qsizetype end = std::min<qsizetype>(to, chars.size());
    quint32 currentLine = std::numeric_limits<quint32>::max();
    for (qsizetype i = from; i < end; ++i) {
        const TextChar& c = chars.at(i);
        if (!c.hasBox()) {
            continue;
        }
        if (!out.isEmpty() && c.line == currentLine) {
            out.last() = out.last().united(c.rect);
        } else {
            out.append(c.rect);
            currentLine = c.line;
        }
    }
    return out;
}

QString PageText::text(quint32 from, quint32 to) const {
    QString out;
    const qsizetype end = std::min<qsizetype>(to, chars.size());
    for (qsizetype i = from; i < end; ++i) {
        const char32_t ch = chars.at(i).ch;
        if (ch == U'\r') {
            // A break is "\r\n" or "\r" in the file's text; copy it as a single "\n".
            if (i + 1 < end && chars.at(i + 1).ch == U'\n') {
                ++i;
            }
            out.append(QLatin1Char('\n'));
        } else {
            out.append(QString::fromUcs4(&ch, 1));
        }
    }
    return out;
}

// ---- TextLayer ----

TextLayer::TextLayer(EngineSession* session, CanvasController* controller, QObject* parent)
    : QObject(parent), m_session(session), m_controller(controller) {
    connect(m_session, &EngineSession::opened, this, &TextLayer::onOpened);
    connect(m_session, &EngineSession::textReady, this, &TextLayer::onText);
    connect(m_session, &EngineSession::requestFailed, this,
            [this](quint64 request, const QString&) { onRequestFailed(request); });
    connect(m_session, &EngineSession::engineCrashed, this, &TextLayer::onEngineCrashed);
    connect(m_controller, &CanvasController::viewChanged, this, [this] { viewMoved(); });
    connect(m_controller, &CanvasController::contentChanged, this, [this] { viewMoved(); });
}

void TextLayer::reset() {
    m_pages.clear();
    m_requests.clear();
}

void TextLayer::viewMoved() {
    if (!m_queued) {
        m_queued = true;
        QMetaObject::invokeMethod(this, &TextLayer::wantVisible, Qt::QueuedConnection);
    }
}

void TextLayer::onOpened() {
    viewMoved();
}

void TextLayer::wantVisible() {
    m_queued = false;
    const QVector<PageDraw> visible = m_controller->visiblePages();
    for (const PageDraw& page : visible) {
        if (page.page < m_controller->pageCount() && !m_pages.contains(page.page)) {
            askFor(page.page);
        }
    }
    evict(visible);
}

void TextLayer::ensure(quint32 pageIndex) {
    if (pageIndex < m_controller->pageCount() && !m_pages.contains(pageIndex)) {
        askFor(pageIndex);
    }
}

void TextLayer::askFor(quint32 pageIndex) {
    Entry& entry = m_pages[pageIndex];
    if (entry.request != 0 || entry.text.complete) {
        return;
    }
    const quint64 id = m_session->requestTextPage(
        pageIndex, static_cast<quint32>(entry.text.chars.size()), kPerRequest);
    if (id == 0) {
        m_pages.remove(pageIndex); // the engine is down: asked again when it is back
        return;
    }
    entry.request = id;
    m_requests.insert(id, pageIndex);
}

void TextLayer::evict(const QVector<PageDraw>& visible) {
    if (m_pages.size() <= kMaxCachedPages) {
        return;
    }
    QList<quint32> onScreen;
    for (const PageDraw& page : visible) {
        onScreen.append(page.page);
    }
    const quint32 centre = onScreen.isEmpty() ? 0 : onScreen.first();
    QList<quint32> candidates;
    for (auto it = m_pages.cbegin(); it != m_pages.cend(); ++it) {
        if (!onScreen.contains(it.key()) && it->request == 0) {
            candidates.append(it.key());
        }
    }
    std::sort(candidates.begin(), candidates.end(), [centre](quint32 a, quint32 b) {
        const auto distance = [centre](quint32 p) { return p > centre ? p - centre : centre - p; };
        return distance(a) > distance(b);
    });
    for (const quint32 page : candidates) {
        if (m_pages.size() <= kMaxCachedPages) {
            break;
        }
        m_pages.remove(page);
    }
}

void TextLayer::onText(quint64 request, quint32 page, quint32 skip, quint32 total,
                       const QList<TextChar>& chars) {
    const auto asked = m_requests.constFind(request);
    if (asked == m_requests.constEnd()) {
        return; // not ours, or forgotten by reset
    }
    const quint32 expected = *asked;
    m_requests.remove(request);
    const auto found = m_pages.find(expected);
    if (found == m_pages.end() || expected != page) {
        return;
    }
    found->request = 0;
    PageText& text = found->text;
    // Windows arrive in order; one that does not continue the text is not trusted.
    if (static_cast<quint32>(text.chars.size()) == skip) {
        text.chars.append(chars);
        text.total = total;
    }
    if (static_cast<quint32>(text.chars.size()) >= text.total || chars.isEmpty()) {
        text.finish();
    } else {
        askFor(expected);
    }
    emit textChanged(expected);
}

void TextLayer::onRequestFailed(quint64 request) {
    const auto asked = m_requests.constFind(request);
    if (asked == m_requests.constEnd()) {
        return;
    }
    const quint32 page = *asked;
    m_requests.remove(request);
    const auto found = m_pages.find(page);
    if (found != m_pages.end()) {
        // A page whose text cannot be read has what it has; it is not asked for again.
        found->request = 0;
        found->text.finish();
        emit textChanged(page);
    }
}

void TextLayer::onEngineCrashed(const QString&, bool, const QList<quint64>& lost) {
    for (const quint64 id : lost) {
        const auto asked = m_requests.constFind(id);
        if (asked != m_requests.constEnd()) {
            m_pages.remove(*asked);
            m_requests.remove(id);
        }
    }
}

const PageText* TextLayer::page(quint32 pageIndex) const {
    const auto found = m_pages.constFind(pageIndex);
    return found == m_pages.constEnd() ? nullptr : &found->text;
}

bool TextLayer::isComplete(quint32 pageIndex) const {
    const PageText* text = page(pageIndex);
    return text != nullptr && text->complete;
}

} // namespace vellora
