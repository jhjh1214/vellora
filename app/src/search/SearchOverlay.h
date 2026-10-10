// Paints the hits of a search over the pages on screen: every hit translucent yellow, the current
// one stronger and orange. A transparent widget above the GPU canvas that never takes the mouse,
// like the selection overlay (which sits above this one, so a selection stays visible on a hit).
#pragma once

#include "canvas/CanvasController.h"
#include "search/SearchController.h"

#include <QWidget>

namespace vellora {

class SearchOverlay : public QWidget {
    Q_OBJECT

public:
    explicit SearchOverlay(CanvasController* controller, QWidget* parent = nullptr);

    // The search to paint; null paints nothing. Not owned.
    void setSearch(SearchController* search);

    struct Highlight {
        QRectF rect; // viewport pixels
        bool current = false;
    };
    // What is painted now (for tests and the accessibility tree).
    QList<Highlight> highlights() const;

protected:
    void paintEvent(QPaintEvent* event) override;

private:
    CanvasController* m_controller;
    SearchController* m_search = nullptr;
};

} // namespace vellora
