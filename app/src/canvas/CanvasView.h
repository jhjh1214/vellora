// The scrolling view of a document: a scroll area whose viewport is the GPU canvas. It owns the
// controller, keeps the scroll bars in step with it, and turns Ctrl+wheel into zoom. Keys that
// scroll (arrows, Page Up/Down, Home/End) come from QAbstractScrollArea itself, except that Page
// Up/Down turn pages when the view shows one row at a time. Holding Z (or the "Zoom to Selection"
// command) and dragging a rectangle zooms to it.
#pragma once

#include "canvas/CanvasController.h"
#include "canvas/LinkLayer.h"
#include "text/CaretBrowser.h"
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
class LinkFocusOverlay;
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
    // The text cursor of caret browsing (F7), moved with the arrow keys while it is on.
    CaretBrowser* caret() { return &m_caret; }
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
    // How a link reads to a screen reader and in a tool tip (the describer, else the default).
    QString describeLink(const Link& link) const;
    // How a page is named to a screen reader and in announcements ("Page iv of 12"); the default
    // uses the page number. The owner replaces it to use the document's page labels.
    using PageDescriber = std::function<QString(quint32 page)>;
    void setPageDescriber(PageDescriber describer) { m_pageDescriber = std::move(describer); }
    QString describePage(quint32 page) const;

    // The link a screen reader (or `focusLink`) has put the focus on, if any. A focused link is
    // brought into view, and Enter or Space follows it; Escape lets it go.
    struct LinkRef {
        quint32 page = 0;
        int index = 0;
        friend bool operator==(const LinkRef&, const LinkRef&) = default;
    };
    std::optional<LinkRef> focusedLink() const { return m_focusLink; }
    // Focuses link `index` of `page` (false if there is no such link yet).
    bool focusLink(quint32 page, int index);
    // Moves the focus to the next (or previous) link in reading order, across pages: from the
    // focused link, else from the page on screen. The links of a page not yet read are asked for
    // and the step is made when they arrive. At most `kMaxLinkScan` pages are looked at; false if
    // there is no such link (nothing moves).
    static constexpr quint32 kMaxLinkScan = 64;
    bool focusNextLink(bool forward);
    LinkFocusOverlay* linkFocusOverlay() { return m_linkFocus; }
    void clearLinkFocus();
    // Follows the focused link, as a click on it does.
    bool activateFocusedLink();
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
    // The focused link changed or was let go.
    void linkFocusChanged();

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
    // Makes the step in `m_pendingLinkStep` if the links it needs are here.
    void stepLink();
    // Caret browsing: a key that moves the caret; false if the key is not one.
    bool handleCaretKey(QKeyEvent* event);
    // Tells a screen reader where the caret went.
    void announceCaret();
    // Tells a screen reader that the page on screen changed.
    void announcePage(quint32 page);

    CanvasController m_controller;
    LinkLayer m_links;
    TextLayer m_text;
    TextSelection m_selection;
    CaretBrowser m_caret;
    CanvasWidget* m_canvas;
    QLabel* m_banner = nullptr;
    bool m_syncing = false;
    bool m_zKeyDown = false;
    bool m_zoomRectOneShot = false;
    QRubberBand* m_band = nullptr;
    QPoint m_dragStart;
    bool m_dragging = false;
    LinkDescriber m_describer;
    PageDescriber m_pageDescriber;
    std::optional<LinkRef> m_focusLink;
    LinkFocusOverlay* m_linkFocus = nullptr;
    // A step to the next (true) or previous (false) link waiting for a page's links.
    std::optional<bool> m_pendingLinkStep;
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
