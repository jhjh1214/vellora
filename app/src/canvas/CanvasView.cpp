#include "canvas/CanvasView.h"

#include "accessibility/CanvasAccessible.h"
#include "canvas/CanvasWidget.h"
#include "canvas/LinkFocusOverlay.h"
#include "links/LinkActions.h"
#include "search/SearchOverlay.h"
#include "text/SelectionOverlay.h"

#include <QAccessible>
#include <QApplication>
#include <QClipboard>
#include <QEvent>
#include <QGuiApplication>
#include <QHelpEvent>
#include <QKeyEvent>
#include <QLabel>
#include <QMouseEvent>
#include <QRubberBand>
#include <QScrollBar>
#include <QToolTip>
#include <QWheelEvent>
#include <algorithm>
#include <cmath>
#include <limits>

namespace vellora {

namespace {

constexpr int kSingleStep = 40; // logical pixels per arrow key press or wheel line

// Scroll bars count in `int`; a column of ten thousand pages at 800% zoom is far below the limit.
int clampToInt(double value) {
    constexpr double kLimit = std::numeric_limits<int>::max() / 2.0;
    return static_cast<int>(std::clamp(value, 0.0, kLimit));
}

} // namespace

CanvasView::CanvasView(EngineSession* session, QWidget* parent)
    : QAbstractScrollArea(parent), m_controller(session), m_links(session, &m_controller, this),
      m_text(session, &m_controller, this), m_selection(&m_text, this),
      m_caret(&m_text, &m_selection, &m_controller, this), m_canvas(nullptr) {
    // The canvas is a child of the viewport, not the viewport itself: the scroll area consumes the
    // viewport's paint events, so a QRhiWidget used as viewport would never be asked to render.
    m_canvas = new CanvasWidget(session, &m_controller, viewport());
    // The hits of a search are under the selection, which stays visible on a hit.
    m_searchOverlay = new SearchOverlay(&m_controller, viewport());
    m_overlay = new SelectionOverlay(&m_controller, &m_selection, viewport());
    m_overlay->setCaretBrowser(&m_caret);
    m_linkFocus = new LinkFocusOverlay(this, viewport());
    connect(&m_links, &LinkLayer::linksChanged, this, [this] { stepLink(); });
    connect(&m_caret, &CaretBrowser::changed, this, [this] { announceCaret(); });
    m_banner = new QLabel(viewport());
    m_banner->setTextFormat(Qt::PlainText); // engine-derived text must never be read as markup
    m_banner->setAttribute(Qt::WA_TransparentForMouseEvents);
    m_banner->setStyleSheet(QStringLiteral("background: rgba(32, 32, 32, 220); color: white;"
                                           "padding: 6px 14px; border-radius: 4px;"));
    m_banner->setWordWrap(true);
    m_banner->hide();
    setFocusPolicy(Qt::StrongFocus);
    // Hover: the pointer shows what is under it without a button down.
    viewport()->setMouseTracking(true);
    m_canvas->setMouseTracking(true);
    // Always shown (disabled when there is nothing to scroll): a scroll bar that comes and goes as
    // the zoom or the page changes resizes the viewport, and with it the GPU surface, which costs
    // a frame (measured: 8-9 ms in a turned view, whose pages overflow at 125% but not at 100%).
    setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOn);
    setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOn);
    verticalScrollBar()->setSingleStep(kSingleStep);
    horizontalScrollBar()->setSingleStep(kSingleStep);
    m_controller.setDevicePixelRatio(m_canvas->devicePixelRatioF());

    const auto scrolled = [this] {
        if (!m_syncing) {
            m_controller.setScrollPosition({static_cast<double>(horizontalScrollBar()->value()),
                                            static_cast<double>(verticalScrollBar()->value())});
        }
    };
    connect(verticalScrollBar(), &QScrollBar::valueChanged, this, scrolled);
    connect(horizontalScrollBar(), &QScrollBar::valueChanged, this, scrolled);
    connect(&m_controller, &CanvasController::contentChanged, this, &CanvasView::syncScrollBars);
    connect(&m_controller, &CanvasController::viewModeChanged, this, [this] { syncScrollBars(); });
    connect(&m_controller, &CanvasController::viewChanged, this, &CanvasView::syncScrollBars);
    connect(&m_controller, &CanvasController::currentPageChanged, this,
            [this](quint32 page, quint32) { announcePage(page); });
    installCanvasAccessibility();
    connect(&m_controller, &CanvasController::viewChanged, this, [this] { refreshHover(); });
    connect(&m_links, &LinkLayer::linksChanged, this, [this] { refreshHover(); });
    connect(&m_text, &TextLayer::textChanged, this, [this] { refreshHover(); });
    m_autoScroll.setInterval(30);
    connect(&m_autoScroll, &QTimer::timeout, this, [this] { dragSelection(); });
}

std::optional<CanvasView::PagePoint> CanvasView::pagePointAt(QPointF viewportPos,
                                                             bool nearest) const {
    const QVector<PageDraw> pages = m_controller.visiblePages();
    const PageDraw* best = nullptr;
    double bestDistance = 0.0;
    for (const PageDraw& page : pages) {
        const QRectF& r = page.rect;
        const double dx = viewportPos.x() < r.left()    ? r.left() - viewportPos.x()
                          : viewportPos.x() > r.right() ? viewportPos.x() - r.right()
                                                        : 0.0;
        const double dy = viewportPos.y() < r.top()      ? r.top() - viewportPos.y()
                          : viewportPos.y() > r.bottom() ? viewportPos.y() - r.bottom()
                                                         : 0.0;
        const double distance = dx * dx + dy * dy;
        if (best == nullptr || distance < bestDistance) {
            best = &page;
            bestDistance = distance;
        }
    }
    if (best == nullptr || (bestDistance > 0.0 && !nearest)) {
        return std::nullopt;
    }
    // The point in the page as shown: the inverse of how a page rectangle is placed.
    const QRectF here = PageLayout::toPage(QRectF(viewportPos - best->rect.topLeft(), QSizeF()),
                                           best->filePoints, best->rotation, best->zoom);
    return PagePoint{best->page, here.topLeft()};
}

bool CanvasView::isOverText(QPointF viewportPos) const {
    const std::optional<PagePoint> at = pagePointAt(viewportPos);
    if (!at) {
        return false;
    }
    const PageText* text = m_text.page(at->page);
    return text != nullptr && text->isOverText(at->point);
}

void CanvasView::copySelection() {
    m_selection.copyText([this](const QString& text) {
        QGuiApplication::clipboard()->setText(text);
        emit copied(static_cast<int>(text.size()));
    });
}

void CanvasView::setSearch(SearchController* search) {
    m_searchOverlay->setSearch(search);
}

void CanvasView::reset() {
    m_focusLink.reset();
    m_pendingLinkStep.reset();
    m_caret.reset();
    m_selection.clear();
    m_text.reset();
    m_links.reset();
    m_controller.reset();
}

void CanvasView::syncScrollBars() {
    // Setting ranges and values from the controller must not read back as the user scrolling.
    m_syncing = true;
    const QSizeF content = m_controller.contentSize();
    const QSize view = viewport()->size();
    // The vertical range is the whole column when scrolling continuously, else the row shown.
    const CanvasController::VerticalRange range = m_controller.verticalRange();
    verticalScrollBar()->setRange(clampToInt(range.min), clampToInt(range.max));
    verticalScrollBar()->setPageStep(view.height());
    horizontalScrollBar()->setRange(0, clampToInt(content.width() - view.width()));
    horizontalScrollBar()->setPageStep(view.width());
    const QPointF scroll = m_controller.scrollPosition();
    verticalScrollBar()->setValue(clampToInt(scroll.y()));
    horizontalScrollBar()->setValue(clampToInt(scroll.x()));
    m_syncing = false;
}

bool CanvasView::viewportEvent(QEvent* event) {
    if (event->type() == QEvent::Resize) {
        // Also when a scroll bar appears or disappears, which resizes only the viewport.
        m_canvas->setGeometry(viewport()->rect());
        m_searchOverlay->setGeometry(viewport()->rect());
        m_overlay->setGeometry(viewport()->rect());
        placeBanner();
        m_controller.setViewportSize(viewport()->size());
        syncScrollBars();
    }
    if (event->type() == QEvent::ToolTip) {
        const auto* help = static_cast<QHelpEvent*>(event);
        const QString text = linkToolTipAt(help->pos());
        if (text.isEmpty()) {
            QToolTip::hideText();
        } else {
            // Qt reads a tool tip as rich text when it looks like it, and the text is the
            // document's: escaped, and kept as written.
            QToolTip::showText(
                help->globalPos(),
                QStringLiteral("<p style='white-space:pre'>%1</p>").arg(text.toHtmlEscaped()),
                viewport());
        }
        return true;
    }
    if (event->type() == QEvent::Leave) {
        m_pointer.reset();
    }
    return QAbstractScrollArea::viewportEvent(event);
}

QString CanvasView::describeLink(const Link& link) const {
    if (m_describer) {
        return m_describer(link);
    }
    return LinkActions::describe(link, [](quint32 page) { return QString::number(page + 1); });
}

QString CanvasView::describePage(quint32 page) const {
    if (m_pageDescriber) {
        return m_pageDescriber(page);
    }
    return tr("Page %1 of %2").arg(page + 1).arg(m_controller.pageCount());
}

QString CanvasView::linkToolTipAt(QPoint viewportPos) const {
    const LinkLayer::Hit hit = m_links.hitTest(viewportPos);
    if (hit.link == nullptr) {
        return {};
    }
    return describeLink(*hit.link);
}

bool CanvasView::focusLink(quint32 page, int index) {
    const QList<Link> links = m_links.linksOf(page);
    if (index < 0 || index >= links.size()) {
        return false;
    }
    m_focusLink = LinkRef{page, index};
    m_controller.revealRect(page, links.at(index).rect);
    viewport()->update();
    emit linkFocusChanged();
    if (QAccessible::isActive()) {
        if (QAccessibleInterface* link = canvasLinkInterface(this, page, index)) {
            QAccessibleEvent event(link, QAccessible::Focus);
            QAccessible::updateAccessibility(&event);
        }
    }
    return true;
}

void CanvasView::clearLinkFocus() {
    m_pendingLinkStep.reset();
    if (m_focusLink) {
        m_focusLink.reset();
        viewport()->update();
        emit linkFocusChanged();
    }
}

bool CanvasView::focusNextLink(bool forward) {
    m_pendingLinkStep = forward;
    stepLink();
    return true;
}

void CanvasView::stepLink() {
    if (!m_pendingLinkStep) {
        return;
    }
    const bool forward = *m_pendingLinkStep;
    const quint32 count = m_controller.pageCount();
    if (count == 0) {
        m_pendingLinkStep.reset();
        return;
    }
    // Where to start: just after (or before) the focused link, else at the page on screen.
    qint64 page = m_focusLink ? m_focusLink->page : m_controller.currentPage();
    int from = m_focusLink ? m_focusLink->index : (forward ? -1 : std::numeric_limits<int>::max());
    for (quint32 scanned = 0; scanned < kMaxLinkScan && page >= 0 && page < count; ++scanned) {
        const auto current = static_cast<quint32>(page);
        if (!m_links.isComplete(current)) {
            m_links.ensure(current); // the step is made when the links arrive
            return;
        }
        const QList<Link> links = m_links.linksOf(current);
        const int size = static_cast<int>(links.size());
        const int next = forward ? from + 1 : std::min(from, size) - 1;
        if (next >= 0 && next < size) {
            m_pendingLinkStep.reset();
            focusLink(current, next);
            return;
        }
        page += forward ? 1 : -1;
        from = forward ? -1 : std::numeric_limits<int>::max();
    }
    m_pendingLinkStep.reset(); // no further link
}

bool CanvasView::activateFocusedLink() {
    if (!m_focusLink) {
        return false;
    }
    const QList<Link> links = m_links.linksOf(m_focusLink->page);
    if (m_focusLink->index >= links.size()) {
        return false;
    }
    emit linkActivated(links.at(m_focusLink->index));
    return true;
}

void CanvasView::announceCaret() {
    const auto caret = m_caret.caret();
    if (!caret) {
        return;
    }
    if (QAccessibleInterface* page = canvasPageInterface(this, caret->page)) {
        if (QAccessibleTextInterface* text = page->textInterface()) {
            QAccessibleTextCursorEvent event(page, text->cursorPosition());
            QAccessible::updateAccessibility(&event);
        }
    }
}

void CanvasView::announcePage(quint32 page) {
    // Qt drops the event itself when no assistive technology is listening.
    QAccessibleAnnouncementEvent event(this, describePage(page));
    event.setPoliteness(QAccessible::AnnouncementPoliteness::Polite);
    QAccessible::updateAccessibility(&event);
}

Qt::CursorShape CanvasView::linkCursorAt(QPoint viewportPos) const {
    const LinkLayer::Hit hit = m_links.hitTest(viewportPos);
    if (hit.link == nullptr) {
        return Qt::ArrowCursor;
    }
    switch (hit.link->kind) {
    case LinkKind::GoTo:
    case LinkKind::Uri:
    case LinkKind::Named:
        return Qt::PointingHandCursor;
    case LinkKind::Inert:
        return Qt::ForbiddenCursor;
    default:
        return Qt::ArrowCursor; // a link that goes nowhere
    }
}

void CanvasView::refreshHover() {
    if (m_pointer) {
        updateCursor();
    }
}

void CanvasView::wheelEvent(QWheelEvent* event) {
    if (event->modifiers().testFlag(Qt::ControlModifier)) {
        const double notches = event->angleDelta().y() / 120.0;
        if (notches != 0.0) {
            m_controller.zoomBy(std::pow(kWheelZoomPerNotch, notches), event->position());
        }
        event->accept();
        return;
    }
    if (!m_controller.viewMode().continuous) {
        // Turning pages: scrolling past the end of a row shows the next one (the controller
        // decides), which the scroll bar alone cannot do.
        QPointF delta;
        if (!event->pixelDelta().isNull()) {
            delta = -QPointF(event->pixelDelta());
        } else {
            constexpr double kLinesPerNotch = 3.0;
            delta = -QPointF(event->angleDelta()) / 120.0 * kLinesPerNotch * kSingleStep;
        }
        m_controller.scrollBy(delta);
        event->accept();
        return;
    }
    QAbstractScrollArea::wheelEvent(event);
}

void CanvasView::keyPressEvent(QKeyEvent* event) {
    if (m_caret.isOn() && handleCaretKey(event)) {
        event->accept();
        return;
    }
    if (m_focusLink && event->modifiers() == Qt::NoModifier) {
        if (event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter ||
            event->key() == Qt::Key_Space) {
            activateFocusedLink();
            event->accept();
            return;
        }
        if (event->key() == Qt::Key_Escape) {
            clearLinkFocus();
            event->accept();
            return;
        }
    }
    if (event->key() == Qt::Key_Escape && !m_selection.isEmpty()) {
        m_selection.clear();
        event->accept();
        return;
    }
    if (event->key() == Qt::Key_Z && event->modifiers() == Qt::NoModifier) {
        if (!event->isAutoRepeat()) {
            m_zKeyDown = true;
            updateCursor();
        }
        event->accept();
        return;
    }
    if (event->key() == Qt::Key_Escape && (m_dragging || zoomRectArmed())) {
        endDrag(false);
        armZoomRect(false);
        event->accept();
        return;
    }
    if (!m_controller.viewMode().continuous && event->modifiers() == Qt::NoModifier &&
        (event->key() == Qt::Key_PageDown || event->key() == Qt::Key_PageUp)) {
        // A page of scrolling, and at the end of the row, the next row.
        const double page = std::max(1.0, viewport()->height() - 2.0 * kSingleStep);
        m_controller.scrollBy(QPointF(0.0, event->key() == Qt::Key_PageDown ? page : -page));
        event->accept();
        return;
    }
    QAbstractScrollArea::keyPressEvent(event);
}

bool CanvasView::handleCaretKey(QKeyEvent* event) {
    const Qt::KeyboardModifiers modifiers =
        event->modifiers() & ~(Qt::KeypadModifier | Qt::ShiftModifier);
    const bool extend = event->modifiers().testFlag(Qt::ShiftModifier);
    const bool word = modifiers == Qt::ControlModifier;
    if (modifiers != Qt::NoModifier && !word) {
        return false;
    }
    using Move = CaretBrowser::Move;
    switch (event->key()) {
    case Qt::Key_Left:
        return m_caret.move(word ? Move::WordLeft : Move::Left, extend);
    case Qt::Key_Right:
        return m_caret.move(word ? Move::WordRight : Move::Right, extend);
    case Qt::Key_Up:
        return !word && m_caret.move(Move::Up, extend);
    case Qt::Key_Down:
        return !word && m_caret.move(Move::Down, extend);
    case Qt::Key_Home:
        return m_caret.move(word ? Move::DocumentStart : Move::LineStart, extend);
    case Qt::Key_End:
        return m_caret.move(word ? Move::DocumentEnd : Move::LineEnd, extend);
    default:
        return false;
    }
}

void CanvasView::keyReleaseEvent(QKeyEvent* event) {
    if (event->key() == Qt::Key_Z && !event->isAutoRepeat()) {
        m_zKeyDown = false;
        updateCursor();
        event->accept();
        return;
    }
    QAbstractScrollArea::keyReleaseEvent(event);
}

void CanvasView::focusOutEvent(QFocusEvent* event) {
    // The release of Z would go to whoever has the focus now.
    m_zKeyDown = false;
    endDrag(false);
    updateCursor();
    QAbstractScrollArea::focusOutEvent(event);
}

void CanvasView::armZoomRect(bool armed) {
    m_zoomRectOneShot = armed;
    updateCursor();
}

QRect CanvasView::dragRect() const {
    return m_dragging && m_band != nullptr ? m_band->geometry() : QRect();
}

void CanvasView::updateCursor() {
    if (zoomRectArmed()) {
        viewport()->setCursor(Qt::CrossCursor);
    } else {
        Qt::CursorShape shape = Qt::ArrowCursor;
        if (m_pointer) {
            shape = linkCursorAt(*m_pointer);
            // A link wins; otherwise text shows the text cursor.
            if (shape == Qt::ArrowCursor && isOverText(*m_pointer)) {
                shape = Qt::IBeamCursor;
            }
        }
        viewport()->setCursor(shape);
    }
}

void CanvasView::mousePressEvent(QMouseEvent* event) {
    if (event->button() == Qt::LeftButton && zoomRectArmed()) {
        m_dragging = true;
        m_dragStart = event->position().toPoint();
        if (m_band == nullptr) {
            m_band = new QRubberBand(QRubberBand::Rectangle, viewport());
        }
        m_band->setGeometry(QRect(m_dragStart, QSize()));
        m_band->show();
        event->accept();
        return;
    }
    if (event->button() == Qt::LeftButton) {
        const LinkLayer::Hit hit = m_links.hitTest(event->position());
        if (hit.link != nullptr) {
            m_pressedLink = *hit.link;
            m_pressedPage = hit.page;
            m_pressPos = event->position().toPoint();
        } else {
            m_pressedLink.reset();
            startSelection(event);
        }
    }
    QAbstractScrollArea::mousePressEvent(event);
}

void CanvasView::mouseDoubleClickEvent(QMouseEvent* event) {
    // The second press of a double click arrives as its own event. It is at least a second click
    // (the first may not have been seen, as in a synthesized double click).
    m_minimumClicks = 2;
    mousePressEvent(event);
    m_minimumClicks = 1;
}

void CanvasView::startSelection(QMouseEvent* event) {
    const QPoint pos = event->position().toPoint();
    // Presses close in time and place are a double click, a triple click, and so on.
    if (m_clickClock.isValid() && m_clickClock.elapsed() <= QApplication::doubleClickInterval() &&
        (pos - m_clickPos).manhattanLength() <= QApplication::startDragDistance()) {
        m_clicks = std::min(m_clicks + 1, 3);
    } else {
        m_clicks = 1;
    }
    m_clicks = std::max(m_clicks, m_minimumClicks);
    m_clickClock.start();
    m_clickPos = pos;

    const std::optional<PagePoint> at = pagePointAt(event->position());
    if (!at) {
        m_selection.clear();
        return;
    }
    const TextSelection::Unit unit = m_clicks == 1   ? TextSelection::Unit::Character
                                     : m_clicks == 2 ? TextSelection::Unit::Word
                                                     : TextSelection::Unit::Line;
    const bool extend = event->modifiers().testFlag(Qt::ShiftModifier);
    m_selecting = m_selection.begin(at->page, at->point, unit, extend);
    if (m_caret.isOn()) {
        m_caret.placeAt(at->page, at->point);
    }
}

void CanvasView::dragSelection() {
    if (!m_selecting || !m_pointer) {
        m_autoScroll.stop();
        return;
    }
    // Beyond the top or bottom the view scrolls toward the pointer, faster the further it is.
    constexpr double kFastest = 80.0;
    const double y = m_pointer->y();
    const double height = viewport()->height();
    const double outside = y < 0 ? y : (y > height ? y - height : 0.0);
    if (outside != 0.0) {
        m_controller.scrollBy({0.0, std::clamp(outside, -kFastest, kFastest)});
        m_autoScroll.start();
    } else {
        m_autoScroll.stop();
    }
    if (const std::optional<PagePoint> at = pagePointAt(*m_pointer, true)) {
        m_selection.extendTo(at->page, at->point);
    }
}

void CanvasView::mouseMoveEvent(QMouseEvent* event) {
    if (m_dragging) {
        m_band->setGeometry(QRect(m_dragStart, event->position().toPoint()).normalized());
        event->accept();
        return;
    }
    m_pointer = event->position().toPoint();
    updateCursor();
    if (m_selecting && (event->buttons() & Qt::LeftButton)) {
        dragSelection();
    }
    QAbstractScrollArea::mouseMoveEvent(event);
}

void CanvasView::mouseReleaseEvent(QMouseEvent* event) {
    if (m_selecting && event->button() == Qt::LeftButton) {
        // The end of the drag is where the button went up.
        m_pointer = event->position().toPoint();
        dragSelection();
    }
    if (m_dragging && event->button() == Qt::LeftButton) {
        m_band->setGeometry(QRect(m_dragStart, event->position().toPoint()).normalized());
        endDrag(true);
        event->accept();
        return;
    }
    if (event->button() == Qt::LeftButton) {
        m_selecting = false;
        m_autoScroll.stop();
    }
    if (event->button() == Qt::LeftButton && m_pressedLink) {
        // A click: released on the same link it went down on, without being dragged away.
        const Link pressed = *m_pressedLink;
        m_pressedLink.reset();
        const LinkLayer::Hit hit = m_links.hitTest(event->position());
        constexpr int kClickSlop = 6;
        if (hit.link != nullptr && hit.page == m_pressedPage && hit.link->rect == pressed.rect &&
            hit.link->kind == pressed.kind &&
            (event->position().toPoint() - m_pressPos).manhattanLength() <= kClickSlop) {
            emit linkActivated(pressed);
        }
    }
    QAbstractScrollArea::mouseReleaseEvent(event);
}

void CanvasView::endDrag(bool zoom) {
    if (!m_dragging) {
        return;
    }
    m_dragging = false;
    const QRect rect = m_band->geometry();
    m_band->hide();
    // A drag of a few pixels is a click, not a rectangle.
    constexpr int kSmallest = 6;
    if (zoom && rect.width() >= kSmallest && rect.height() >= kSmallest) {
        m_controller.zoomToRect(QRectF(rect));
    }
    // The tool serves one drag when it was armed by the command; the key keeps it while down.
    m_zoomRectOneShot = false;
    updateCursor();
}

void CanvasView::showBanner(const QString& text) {
    m_banner->setText(text);
    m_banner->adjustSize();
    placeBanner();
    m_banner->show();
    m_banner->raise(); // above the canvas
}

void CanvasView::hideBanner() {
    m_banner->hide();
}

QString CanvasView::bannerText() const {
    return m_banner->text();
}

bool CanvasView::bannerVisible() const {
    return !m_banner->isHidden();
}

void CanvasView::placeBanner() {
    constexpr int kTopMargin = 12;
    const int maxWidth = std::max(0, viewport()->width() - 24);
    m_banner->setMaximumWidth(maxWidth);
    m_banner->adjustSize();
    m_banner->move((viewport()->width() - m_banner->width()) / 2, kTopMargin);
}

void CanvasView::zoomIn() {
    m_controller.zoomBy(kZoomStep, QPointF(viewport()->width() / 2.0, viewport()->height() / 2.0));
}

void CanvasView::zoomOut() {
    m_controller.zoomBy(1.0 / kZoomStep,
                        QPointF(viewport()->width() / 2.0, viewport()->height() / 2.0));
}

void CanvasView::actualSize() {
    m_controller.actualSize();
}

void CanvasView::fitWidth() {
    m_controller.fitWidth();
}

void CanvasView::fitPage() {
    m_controller.fitPage();
}

} // namespace vellora
