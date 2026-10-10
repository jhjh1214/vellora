// The scrolling view of a document: a scroll area whose viewport is the GPU canvas. It owns the
// controller, keeps the scroll bars in step with it, and turns Ctrl+wheel into zoom. Keys that
// scroll (arrows, Page Up/Down, Home/End) come from QAbstractScrollArea itself, except that Page
// Up/Down turn pages when the view shows one row at a time. Holding Z (or the "Zoom to Selection"
// command) and dragging a rectangle zooms to it.
#pragma once

#include "canvas/CanvasController.h"
#include "canvas/LinkLayer.h"
#include "text/TextLayer.h"
#include "text/TextSelection.h"

#include <QAbstractScrollArea>
#include <QElapsedTimer>
#include <QTimer>
#include <functional>
#include <optional>

class QLabel;
class QRubberBand;

namespace vellora {

class CanvasWidget;
class SearchController;
class SearchOverlay;
class SelectionOverlay;

class CanvasView : public QAbstractScrollArea {
    Q_OBJECT

public:
    // Ctrl+wheel: one notch (120 units) multiplies the zoom by this much.
    static constexpr double kWheelZoomPerNotch = 1.1;
    // Ctrl+plus / Ctrl+minus multiply the zoom by this much.
    static constexpr double kZoomStep = 1.25;

    explicit CanvasView(EngineSession* session, QWidget* parent = nullptr);

    CanvasController* controller() { return &m_controller; }
    LinkLayer* links() { return &m_links; }
    TextLayer* text() { return &m_text; }
    TextSelection* selection() { return &m_selection; }
    SelectionOverlay* selectionOverlay() { return m_overlay; }
    SearchOverlay* searchOverlay() { return m_searchOverlay; }
    // The search whose hits are painted over the pages (not owned); null for none.
    void setSearch(SearchController* search);

    // A place on a page: the page and a point in points of the page as shown.
    struct PagePoint {
        quint32 page = 0;
        QPointF point;
    };
    // The page under `viewportPos`; with `nearest`, the nearest page when none is under it.
    std::optional<PagePoint> pagePointAt(QPointF viewportPos, bool nearest = false) const;
    // Whether the pointer at `viewportPos` is over a character.
    bool isOverText(QPointF viewportPos) const;
    // Copies the selected text to the clipboard (read from the engine, so it may take a moment;
    // `copied` follows with the number of characters). Nothing if nothing is selected.
    void copySelection();
    CanvasWidget* canvas() { return m_canvas; }

    // Forgets the old document's view; call before the session opens another file.
    void reset();

    // A non-modal notice over the top of the canvas (mouse clicks pass through it), for example
    // while the engine restarts. Plain text.
    void showBanner(const QString& text);
    void hideBanner();
    QString bannerText() const;
    bool bannerVisible() const;

    void zoomIn();
    void zoomOut();
    void actualSize();
    void fitWidth();
    void fitPage();

    // ---- links ----
    // How a link reads in a tool tip; the default names the target page by its number. The owner
    // replaces it to use the document's page labels. Plain text.
    using LinkDescriber = std::function<QString(const Link&)>;
    void setLinkDescriber(LinkDescriber describer) { m_describer = std::move(describer); }
    // The tool tip text of the link under `viewportPos`, or empty if there is none.
    QString linkToolTipAt(QPoint viewportPos) const;
    // The shape the pointer has over `viewportPos`: a hand over a link that can be followed, a
    // "not allowed" sign over one that would run what Vellora never runs, else an arrow.
    Qt::CursorShape linkCursorAt(QPoint viewportPos) const;

    // The rectangle tool: while armed, dragging with the left button draws a rectangle and zooms to
    // it. Held Z arms it for as long as the key is down; `armZoomRect(true)` arms it for one drag.
    void armZoomRect(bool armed);
    bool zoomRectArmed() const { return m_zoomRectOneShot || m_zKeyDown; }
    // The rectangle being dragged (viewport pixels); empty when none.
    QRect dragRect() const;

signals:
    // The reader clicked (pressed and released on) a link. What it does is the owner's to decide.
    void linkActivated(const vellora::Link& link);
    // The selected text is on the clipboard.
    void copied(int characters);

protected:
    bool viewportEvent(QEvent* event) override;
    void wheelEvent(QWheelEvent* event) override;
    void keyPressEvent(QKeyEvent* event) override;
    void keyReleaseEvent(QKeyEvent* event) override;
    void mousePressEvent(QMouseEvent* event) override;
    void mouseMoveEvent(QMouseEvent* event) override;
    void mouseReleaseEvent(QMouseEvent* event) override;
    void mouseDoubleClickEvent(QMouseEvent* event) override;
    void focusOutEvent(QFocusEvent* event) override;

private:
    void syncScrollBars();
    void placeBanner();
    void updateCursor();
    void refreshHover();
    void startSelection(QMouseEvent* event);
    void dragSelection();
    void endDrag(bool zoom);

    CanvasController m_controller;
    LinkLayer m_links;
    TextLayer m_text;
    TextSelection m_selection;
    CanvasWidget* m_canvas;
    QLabel* m_banner = nullptr;
    bool m_syncing = false;
    bool m_zKeyDown = false;
    bool m_zoomRectOneShot = false;
    QRubberBand* m_band = nullptr;
    QPoint m_dragStart;
    bool m_dragging = false;
    LinkDescriber m_describer;
    // Where the pointer is over the viewport, to look again when the view moves beneath it.
    std::optional<QPoint> m_pointer;
    // The link the left button went down on, and where.
    std::optional<Link> m_pressedLink;
    quint32 m_pressedPage = 0;
    QPoint m_pressPos;
    SearchOverlay* m_searchOverlay = nullptr;
    SelectionOverlay* m_overlay = nullptr;
    // Selecting: the button is down on text. Clicks counted for word and line selection.
    bool m_selecting = false;
    QElapsedTimer m_clickClock;
    QPoint m_clickPos;
    int m_clicks = 0;
    int m_minimumClicks = 1;
    // While the pointer is dragged outside the viewport the view scrolls and the selection grows.
    QTimer m_autoScroll;
};

} // namespace vellora
