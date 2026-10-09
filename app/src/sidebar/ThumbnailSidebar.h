// The left sidebar of a document tab: two tabs, the page thumbnails (with a slider for their size
// under them) and the document outline.
#pragma once

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
    enum class Tab { Thumbnails = 0, Outline = 1 };

    ThumbnailSidebar(EngineSession* session, CanvasController* controller,
                     QWidget* parent = nullptr);

    ThumbnailView& thumbnails() { return *m_view; }
    OutlineView& outline() { return *m_outline; }
    QSlider& sizeSlider() { return *m_slider; }

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
};

} // namespace vellora
