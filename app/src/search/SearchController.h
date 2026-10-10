// One search of one document: the query, the hits that have arrived (in page order, as the engine
// sends them), how far the search has got, and which hit is the current one. The engine searches
// page by page and streams hits, so everything here grows while the search runs. A new query ends
// the old one (the engine is told, and answers of the old one are ignored).
//
// The first hit at or after the page the reader was on becomes the current one as soon as it
// arrives; if there is none there, the first hit of the document becomes current when the search
// ends (the search wraps). Stepping with `next` and `previous` wraps too.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasController.h"

#include <QList>
#include <QObject>
#include <QString>
#include <utility>

namespace vellora {

class SearchController : public QObject {
    Q_OBJECT

public:
    // The engine stops after this many hits.
    static constexpr quint32 kMaxHits = 10'000;

    struct Query {
        QString text;
        bool caseSensitive = false;
        bool wholeWord = false;
        bool regex = false;
        friend bool operator==(const Query&, const Query&) = default;
    };
    enum class State {
        Idle,     // no query
        Running,  // the engine is searching
        Finished, // every page was searched
        Limit,    // stopped at `kMaxHits`
        Failed,   // the query was refused or the search could not run: see `error`
    };

    // Neither is owned; both must outlive the controller.
    SearchController(EngineSession* session, CanvasController* controller,
                     QObject* parent = nullptr);

    // Searches for `query`; an empty text is `clear`.
    void start(const Query& query);
    // Ends the search and forgets its hits.
    void clear();

    const Query& query() const { return m_query; }
    State state() const { return m_state; }
    bool isRunning() const { return m_state == State::Running; }
    // What the engine said about a refused query, trimmed to its last line.
    QString error() const { return m_error; }

    const QList<SearchHit>& hits() const { return m_hits; }
    int count() const { return static_cast<int>(m_hits.size()); }
    // Pages searched so far, and the pages the document has.
    quint32 pagesDone() const { return m_pagesDone; }
    quint32 pageCount() const { return m_session->pageCount(); }

    // The current hit, or -1.
    int current() const { return m_current; }
    void setCurrent(int index);
    // Steps to the next or previous hit, wrapping. While the search is running, `next` at the last
    // hit found so far stays where it is (more may come). False if there is no hit to go to.
    bool next();
    bool previous();

    // The hits on `page` are hits()[first, last).
    std::pair<int, int> hitsOnPage(quint32 page) const;

    // One line for the reader: how far the search is, how many results, or why it failed.
    QString statusText() const;

signals:
    // Hits were appended at the end of `hits()`.
    void hitsAdded(int first, int count);
    // The hits were forgotten (a new query, or `clear`).
    void cleared();
    // The current hit changed (-1: none).
    void currentChanged(int index);
    // `state`, `error` or the progress changed.
    void statusChanged();

private slots:
    void onHits(quint64 request, const QList<vellora::SearchHit>& hits, quint32 pagesDone);
    void onDone(quint64 request, vellora::SearchEnd end, quint32 hits, quint32 pagesDone);
    void onRequestFailed(quint64 request, const QString& message);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void onOpened();

private:
    void stopRunning();
    void fail(const QString& error);
    void setState(State state);

    EngineSession* m_session;
    CanvasController* m_controller;
    Query m_query;
    State m_state = State::Idle;
    QString m_error;
    QList<SearchHit> m_hits;
    quint64 m_request = 0;
    quint32 m_pagesDone = 0;
    quint32 m_startPage = 0;
    int m_current = -1;
    // The engine died while this search was running and will be back: search again when it is.
    bool m_resume = false;
};

} // namespace vellora
