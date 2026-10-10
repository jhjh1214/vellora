// The list of the hits of a search, in the sidebar: a line saying how the search is going and, for
// each hit, the page and the words around it with the match in bold. Clicking (or Enter on) a hit
// makes it the current hit. The list follows the search while it runs.
#pragma once

#include "search/SearchController.h"

#include <QAbstractListModel>
#include <QListView>
#include <QWidget>
#include <functional>

class QLabel;

namespace vellora {

class SearchResultsModel : public QAbstractListModel {
    Q_OBJECT

public:
    enum Role {
        PageLabelRole = Qt::UserRole + 1, // the page's label
        BeforeRole,                       // the snippet before the match
        MatchRole,                        // the match
        AfterRole,                        // the snippet after the match
    };
    using PageLabeler = std::function<QString(quint32 page)>;

    // `search` is not owned and must outlive the model.
    explicit SearchResultsModel(SearchController* search, QObject* parent = nullptr);

    // How a page is named (the document's label); the default is its number.
    void setPageLabeler(PageLabeler labeler);
    // The labeler would answer differently now (the document's labels have been read).
    void refreshLabels();

    int rowCount(const QModelIndex& parent = {}) const override;
    QVariant data(const QModelIndex& index, int role = Qt::DisplayRole) const override;

private:
    SearchController* m_search;
    PageLabeler m_labeler;
    // The rows the views know about; the controller may be ahead by one message.
    int m_rows = 0;
};

class SearchResultsView : public QWidget {
    Q_OBJECT

public:
    explicit SearchResultsView(SearchController* search, QWidget* parent = nullptr);

    SearchResultsModel& model() { return m_model; }
    QListView& list() { return *m_list; }
    QLabel& summary() { return *m_summary; }

signals:
    // The reader chose a hit in the list.
    void hitChosen(int index);

private:
    SearchController* m_search;
    SearchResultsModel m_model;
    QLabel* m_summary;
    QListView* m_list;
};

} // namespace vellora
