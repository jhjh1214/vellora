// The left sidebar of a document tab: the thumbnails and, under them, a slider for their size.
#pragma once

#include "sidebar/ThumbnailView.h"

#include <QWidget>

class QSlider;

namespace vellora {

class ThumbnailSidebar : public QWidget {
    Q_OBJECT

public:
    ThumbnailSidebar(EngineSession* session, CanvasController* controller,
                     QWidget* parent = nullptr);

    ThumbnailView& thumbnails() { return *m_view; }
    QSlider& sizeSlider() { return *m_slider; }

    // Forgets the previous document; call before the session opens another file.
    void reset() { m_view->reset(); }

signals:
    // The user changed the size of the thumbnails (slider or Ctrl+wheel).
    void thumbnailWidthChanged(int width);

private:
    ThumbnailView* m_view;
    QSlider* m_slider;
};

} // namespace vellora
