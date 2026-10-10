#include "search/SearchController.h"

#include <algorithm>

namespace vellora {

SearchController::SearchController(EngineSession* session, CanvasController* controller,
                                   QObject* parent)
    : QObject(parent), m_session(session), m_controller(controller) {
    connect(m_session, &EngineSession::searchHits, this, &SearchController::onHits);
    connect(m_session, &EngineSession::searchDone, this, &SearchController::onDone);
    connect(m_session, &EngineSession::requestFailed, this, &SearchController::onRequestFailed);
    connect(m_session, &EngineSession::engineCrashed, this, &SearchController::onEngineCrashed);
    connect(m_session, &EngineSession::opened, this, &SearchController::onOpened);
}

void SearchController::setState(State state) {
    m_state = state;
    emit statusChanged();
}

void SearchController::stopRunning() {
    if (m_request != 0) {
        m_session->cancel(m_request);
        m_request = 0;
    }
    m_resume = false;
}

void SearchController::clear() {
    stopRunning();
    const bool hadHits = !m_hits.isEmpty() || m_current >= 0;
    m_hits.clear();
    m_query = {};
    m_error.clear();
    m_pagesDone = 0;
    const int was = m_current;
    m_current = -1;
    if (hadHits || m_state != State::Idle) {
        emit cleared();
    }
    if (was >= 0) {
        emit currentChanged(-1);
    }
    setState(State::Idle);
}

void SearchController::start(const Query& query) {
    if (query.text.isEmpty()) {
        clear();
        return;
    }
    stopRunning();
    const int was = m_current;
    m_hits.clear();
    m_current = -1;
    m_error.clear();
    m_pagesDone = 0;
    m_query = query;
    m_startPage = m_controller->currentPage();
    emit cleared();
    if (was >= 0) {
        emit currentChanged(-1);
    }
    m_request =
        m_session->requestSearch(query.text, query.caseSensitive, query.wholeWord, query.regex);
    if (m_request == 0) {
        fail(tr("The search could not be started."));
        return;
    }
    setState(State::Running);
}

void SearchController::fail(const QString& error) {
    m_request = 0;
    m_error = error;
    setState(State::Failed);
}

void SearchController::setCurrent(int index) {
    if (index < 0 || index >= m_hits.size() || index == m_current) {
        return;
    }
    m_current = index;
    emit currentChanged(index);
    emit statusChanged();
}

bool SearchController::next() {
    if (m_hits.isEmpty()) {
        return false;
    }
    if (m_current + 1 < m_hits.size()) {
        setCurrent(m_current + 1);
    } else if (!isRunning()) {
        // Wraps; a lone hit is "stepped to" again so that the view returns to it.
        if (m_hits.size() == 1) {
            emit currentChanged(m_current);
        }
        setCurrent(0);
    }
    return true;
}

bool SearchController::previous() {
    if (m_hits.isEmpty()) {
        return false;
    }
    if (m_current > 0) {
        setCurrent(m_current - 1);
    } else if (!isRunning()) {
        if (m_hits.size() == 1) {
            emit currentChanged(m_current);
        }
        setCurrent(static_cast<int>(m_hits.size()) - 1);
    }
    return true;
}

std::pair<int, int> SearchController::hitsOnPage(quint32 page) const {
    const auto byPage = [](const SearchHit& hit, quint32 value) { return hit.page < value; };
    const auto first = std::lower_bound(m_hits.begin(), m_hits.end(), page, byPage);
    auto last = first;
    while (last != m_hits.end() && last->page == page) {
        ++last;
    }
    return {static_cast<int>(first - m_hits.begin()), static_cast<int>(last - m_hits.begin())};
}

void SearchController::onHits(quint64 request, const QList<SearchHit>& hits, quint32 pagesDone) {
    if (request == 0 || request != m_request) {
        return;
    }
    m_pagesDone = pagesDone;
    const int first = static_cast<int>(m_hits.size());
    m_hits.append(hits);
    if (!hits.isEmpty()) {
        emit hitsAdded(first, static_cast<int>(hits.size()));
    }
    if (m_current < 0) {
        for (int i = first; i < m_hits.size(); ++i) {
            if (m_hits.at(i).page >= m_startPage) {
                setCurrent(i);
                break;
            }
        }
    }
    emit statusChanged();
}

void SearchController::onDone(quint64 request, SearchEnd end, quint32 hits, quint32 pagesDone) {
    Q_UNUSED(hits);
    if (request == 0 || request != m_request) {
        return;
    }
    m_request = 0;
    m_pagesDone = pagesDone;
    // No hit at or after the page the reader was on: the search wraps to the first.
    if (m_current < 0 && !m_hits.isEmpty()) {
        setCurrent(0);
    }
    setState(end == SearchEnd::TooManyHits ? State::Limit : State::Finished);
}

void SearchController::onRequestFailed(quint64 request, const QString& message) {
    if (request == 0 || request != m_request) {
        return;
    }
    // A refused expression comes with the parser's whole report; its last line is the reason.
    const QStringList lines = message.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    QString reason = lines.isEmpty() ? message : lines.last().trimmed();
    if (reason.startsWith(QLatin1String("error: "))) {
        reason = reason.mid(7);
    }
    fail(reason);
}

void SearchController::onEngineCrashed(const QString&, bool willRestart,
                                       const QList<quint64>& lost) {
    if (m_request == 0 || !lost.contains(m_request)) {
        return;
    }
    m_request = 0;
    if (willRestart) {
        m_resume = true; // see `onOpened`
    } else {
        fail(tr("The search was interrupted."));
    }
}

void SearchController::onOpened() {
    // Also the answer of an engine that was restarted after a crash: search again from the start.
    if (m_resume) {
        m_resume = false;
        start(m_query);
    }
}

QString SearchController::statusText() const {
    switch (m_state) {
    case State::Idle:
        return {};
    case State::Failed:
        return m_error;
    case State::Running:
        if (m_hits.isEmpty()) {
            return tr("Searching… %1 of %2 pages").arg(m_pagesDone).arg(pageCount());
        }
        return tr("%1 of %n result(s) so far — searching…", nullptr,
                  static_cast<int>(m_hits.size()))
            .arg(m_current + 1);
    case State::Limit:
        return tr("%1 of %2+ results").arg(m_current + 1).arg(kMaxHits);
    case State::Finished:
        if (m_hits.isEmpty()) {
            return tr("No results");
        }
        return tr("%1 of %n result(s)", nullptr, static_cast<int>(m_hits.size()))
            .arg(m_current + 1);
    }
    return {};
}

} // namespace vellora
