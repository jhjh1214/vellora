#include "sidebar/ThumbnailSidebar.h"

#include <QSignalBlocker>
#include <QSlider>
#include <QVBoxLayout>

namespace vellora {

ThumbnailSidebar::ThumbnailSidebar(EngineSession* session, CanvasController* controller,
                                   QWidget* parent)
    : QWidget(parent) {
    m_view = new ThumbnailView(session, controller, this);
    m_slider = new QSlider(Qt::Horizontal, this);
    m_slider->setRange(ThumbnailView::kMinWidth, ThumbnailView::kMaxWidth);
    m_slider->setValue(m_view->thumbnailWidth());
    m_slider->setToolTip(tr("Thumbnail size"));
    m_slider->setAccessibleName(tr("Thumbnail size"));

    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(m_view, 1);
    layout->addWidget(m_slider);
    setFocusProxy(m_view);

    connect(m_slider, &QSlider::valueChanged, m_view, &ThumbnailView::setThumbnailWidth);
    connect(m_view, &ThumbnailView::thumbnailWidthChanged, this, [this](int width) {
        const QSignalBlocker blocker(m_slider);
        m_slider->setValue(width);
        emit thumbnailWidthChanged(width);
    });
}

} // namespace vellora
