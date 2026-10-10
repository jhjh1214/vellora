// The left sidebar of a document tab: the page thumbnails (with a slider for their size under
// them), the document outline and, once `setSearch` gave it a search, the list of search results.
#pragma once

#include "search/SearchResultsView.h"
#include "sidebar/OutlineView.h"
#include "sidebar/ThumbnailView.h"

#include <QWidget>

class QSlider;
class QStackedWidget;
class QTabBar;

namespace vellora {

class ThumbnailSidebar : public QWidget {
    Q_OBJECT

public:
    enum class Tab { Thumbnails = 0, Outline = 1, Search = 2 };

    ThumbnailSidebar(EngineSession* session, CanvasController* controller,
                     QWidget* parent = nullptr);

    ThumbnailView& thumbnails() { return *m_view; }
    OutlineView& outline() { return *m_outline; }
    QSlider& sizeSlider() { return *m_slider; }
    // Adds the Search tab, which lists the hits of `search` (not owned). Once; later calls do
    // nothing.
    void setSearch(SearchController* search);
    // Null until `setSearch`.
    SearchResultsView* results() { return m_results; }

    Tab currentTab() const;
    void setCurrentTab(Tab tab);

    // Forgets the previous document; call before the session opens another file.
    void reset() {
        m_view->reset();
        m_outline->forgetDocument();
    }

signals:
    // The user changed the size of the thumbnails (slider or Ctrl+wheel).
    void thumbnailWidthChanged(int width);
    void currentTabChanged(vellora::ThumbnailSidebar::Tab tab);

private:
    QTabBar* m_tabs;
    QStackedWidget* m_pages;
    ThumbnailView* m_view;
    OutlineView* m_outline;
    QSlider* m_slider;
    SearchResultsView* m_results = nullptr;
};

} // namespace vellora
