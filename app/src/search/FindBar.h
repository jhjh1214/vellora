// The find bar above the canvas: a text field, the options (match case, whole words, regular
// expression), previous and next buttons, a line that says how the search is going, and a button
// that shows the list of results. Typing starts a search after a short pause; Enter steps to the
// next hit, Shift+Enter to the previous one, Escape closes the bar and ends the search.
#pragma once

#include "search/SearchController.h"

#include <QTimer>
#include <QWidget>

class QCheckBox;
class QLabel;
class QLineEdit;
class QToolButton;

namespace vellora {

class FindBar : public QWidget {
    Q_OBJECT

public:
    // How long the text must rest before it is searched for.
    static constexpr int kDebounceMs = 200;

    // `search` is not owned and must outlive the bar.
    explicit FindBar(SearchController* search, QWidget* parent = nullptr);

    // Shows the bar with the focus in the text field and its text selected. With `text`, that is
    // what the field holds (and is searched for at once).
    void open(const QString& text = {});
    // Hides the bar and ends the search.
    void closeBar();
    // As `closeBar`, for a new document: the text is forgotten too and nothing is reported.
    void reset();

    QLineEdit& edit() { return *m_edit; }
    QCheckBox& matchCase() { return *m_case; }
    QCheckBox& wholeWords() { return *m_words; }
    QCheckBox& regex() { return *m_regex; }
    QToolButton& nextButton() { return *m_next; }
    QToolButton& previousButton() { return *m_previous; }
    QToolButton& resultsButton() { return *m_results; }
    QLabel& status() { return *m_status; }

    // The query the controls describe.
    SearchController::Query query() const;
    // Searches for the query now, without waiting for the pause.
    void searchNow();

signals:
    // The reader asked for the list of results.
    void resultsRequested();
    // The bar was closed (Escape or the close button): the focus belongs to the canvas again.
    void closed();

protected:
    bool eventFilter(QObject* watched, QEvent* event) override;

private:
    void queryChanged();
    void refreshStatus();

    SearchController* m_search;
    QLineEdit* m_edit;
    QCheckBox* m_case;
    QCheckBox* m_words;
    QCheckBox* m_regex;
    QToolButton* m_previous;
    QToolButton* m_next;
    QToolButton* m_results;
    QLabel* m_status;
    QTimer m_pause;
};

} // namespace vellora
