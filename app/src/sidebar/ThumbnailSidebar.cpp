#include "sidebar/ThumbnailSidebar.h"

#include <QSignalBlocker>
#include <QSlider>
#include <QStackedWidget>
#include <QTabBar>
#include <QVBoxLayout>

namespace vellora {

ThumbnailSidebar::ThumbnailSidebar(EngineSession* session, CanvasController* controller,
                                   QWidget* parent)
    : QWidget(parent) {
    m_tabs = new QTabBar(this);
    m_tabs->setDrawBase(true);
    m_tabs->setExpanding(true);
    m_tabs->addTab(tr("Pages"));
    m_tabs->addTab(tr("Outline"));
    m_tabs->setAccessibleName(tr("Sidebar"));

    auto* thumbnails = new QWidget(this);
    m_view = new ThumbnailView(session, controller, thumbnails);
    m_slider = new QSlider(Qt::Horizontal, thumbnails);
    m_slider->setRange(ThumbnailView::kMinWidth, ThumbnailView::kMaxWidth);
    m_slider->setValue(m_view->thumbnailWidth());
    m_slider->setToolTip(tr("Thumbnail size"));
    m_slider->setAccessibleName(tr("Thumbnail size"));
    auto* thumbnailLayout = new QVBoxLayout(thumbnails);
    thumbnailLayout->setContentsMargins(0, 0, 0, 0);
    thumbnailLayout->setSpacing(0);
    thumbnailLayout->addWidget(m_view, 1);
    thumbnailLayout->addWidget(m_slider);
    thumbnails->setFocusProxy(m_view);

    m_outline = new OutlineView(session, controller, this);

    m_pages = new QStackedWidget(this);
    m_pages->addWidget(thumbnails);
    m_pages->addWidget(m_outline);

    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(m_tabs);
    layout->addWidget(m_pages, 1);
    setFocusProxy(m_pages);

    connect(m_tabs, &QTabBar::currentChanged, this, [this](int index) {
        m_pages->setCurrentIndex(index);
        emit currentTabChanged(static_cast<Tab>(index));
    });
    connect(m_slider, &QSlider::valueChanged, m_view, &ThumbnailView::setThumbnailWidth);
    connect(m_view, &ThumbnailView::thumbnailWidthChanged, this, [this](int width) {
        const QSignalBlocker blocker(m_slider);
        m_slider->setValue(width);
        emit thumbnailWidthChanged(width);
    });
}

ThumbnailSidebar::Tab ThumbnailSidebar::currentTab() const {
    return static_cast<Tab>(m_tabs->currentIndex());
}

void ThumbnailSidebar::setCurrentTab(Tab tab) {
    m_tabs->setCurrentIndex(static_cast<int>(tab));
}

} // namespace vellora
