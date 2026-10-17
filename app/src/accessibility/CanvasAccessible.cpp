#include "accessibility/CanvasAccessible.h"

#include "canvas/CanvasView.h"

#include <QAccessible>
#include <QAccessibleWidget>
#include <QCoreApplication>
#include <QHash>
#include <QPointer>
#include <algorithm>
#include <memory>
#include <optional>
#include <utility>

namespace vellora {

namespace {

class CanvasAccessible;
class PageAccessible;

QString tr(const char* text) {
    return QCoreApplication::translate("vellora::CanvasAccessible", text);
}

// The text of a page as one string, and where each character starts in it. The characters are
// UTF-32 and the accessibility API counts UTF-16 units, so the two numberings are kept side by
// side.
struct PageString {
    QString text;
    QList<int> starts; // starts[i] is the offset of character i; starts.last() == text.size()
};

PageString pageString(const PageText& page) {
    PageString out;
    out.starts.reserve(page.chars.size() + 1);
    for (const TextChar& c : page.chars) {
        out.starts.append(static_cast<int>(out.text.size()));
        out.text.append(QString::fromUcs4(&c.ch, 1));
    }
    out.starts.append(static_cast<int>(out.text.size()));
    return out;
}

// ---- a link ----

class LinkAccessible : public QAccessibleInterface, public QAccessibleActionInterface {
public:
    LinkAccessible(CanvasAccessible* canvas, PageAccessible* page, quint32 pageIndex, int index)
        : m_canvas(canvas), m_pageInterface(page), m_page(pageIndex), m_index(index) {}

    bool isValid() const override;
    QObject* object() const override { return nullptr; }
    QAccessibleInterface* parent() const override;
    QAccessibleInterface* child(int) const override { return nullptr; }
    int childCount() const override { return 0; }
    int indexOfChild(const QAccessibleInterface*) const override { return -1; }
    QAccessibleInterface* childAt(int, int) const override { return nullptr; }
    QString text(QAccessible::Text t) const override;
    void setText(QAccessible::Text, const QString&) override {}
    QRect rect() const override;
    QAccessible::Role role() const override { return QAccessible::Link; }
    QAccessible::State state() const override;
    void* interface_cast(QAccessible::InterfaceType type) override {
        return type == QAccessible::ActionInterface ? static_cast<QAccessibleActionInterface*>(this)
                                                    : nullptr;
    }

    QStringList actionNames() const override {
        return {QAccessibleActionInterface::pressAction(),
                QAccessibleActionInterface::setFocusAction()};
    }
    void doAction(const QString& name) override;
    QStringList keyBindingsForAction(const QString&) const override { return {}; }

    int index() const { return m_index; }

private:
    CanvasView* view() const;
    std::optional<Link> link() const;

    CanvasAccessible* m_canvas;
    PageAccessible* m_pageInterface;
    quint32 m_page;
    int m_index;
};

// ---- a page ----

class PageAccessible : public QAccessibleInterface, public QAccessibleTextInterface {
public:
    PageAccessible(CanvasAccessible* canvas, quint32 page) : m_canvas(canvas), m_page(page) {}
    ~PageAccessible() override {
        for (LinkAccessible* link : std::as_const(m_links)) {
            QAccessible::deleteAccessibleInterface(QAccessible::uniqueId(link));
        }
    }

    quint32 page() const { return m_page; }
    CanvasView* view() const;
    LinkAccessible* linkInterface(int index);

    // The page's text as far as it has arrived, asking the engine for it the first time.
    const PageString& content() const;
    // Called when more of the text arrived: tells the screen reader what was added.
    void textArrived();

    // QAccessibleInterface
    bool isValid() const override;
    QObject* object() const override { return nullptr; }
    QAccessibleInterface* parent() const override;
    QAccessibleInterface* child(int index) const override {
        return const_cast<PageAccessible*>(this)->linkInterface(index);
    }
    int childCount() const override;
    int indexOfChild(const QAccessibleInterface* child) const override;
    QAccessibleInterface* childAt(int x, int y) const override;
    QString text(QAccessible::Text t) const override;
    void setText(QAccessible::Text, const QString&) override {}
    QRect rect() const override;
    QAccessible::Role role() const override { return QAccessible::Section; }
    QAccessible::State state() const override;
    void* interface_cast(QAccessible::InterfaceType type) override {
        return type == QAccessible::TextInterface ? static_cast<QAccessibleTextInterface*>(this)
                                                  : nullptr;
    }

    // QAccessibleTextInterface
    void selection(int selectionIndex, int* startOffset, int* endOffset) const override;
    int selectionCount() const override;
    void addSelection(int, int) override {}
    void removeSelection(int) override {}
    void setSelection(int, int, int) override {}
    int cursorPosition() const override;
    void setCursorPosition(int position) override;
    QString text(int startOffset, int endOffset) const override;
    int characterCount() const override { return static_cast<int>(content().text.size()); }
    QRect characterRect(int offset) const override;
    int offsetAtPoint(const QPoint& point) const override;
    void scrollToSubstring(int startIndex, int endIndex) override;
    QString attributes(int offset, int* startOffset, int* endOffset) const override;

private:
    // The character a UTF-16 offset is in, or -1.
    int characterAt(int offset) const;
    QRect toGlobal(const QRectF& viewportRect) const;

    CanvasAccessible* m_canvas;
    quint32 m_page;
    mutable PageString m_content;
    mutable qsizetype m_contentChars = -1;
    QHash<int, LinkAccessible*> m_links;
};

// ---- the document ----

class CanvasAccessible : public QAccessibleWidget {
public:
    explicit CanvasAccessible(CanvasView* view) : QAccessibleWidget(view, QAccessible::Document) {
        // Text arrives asynchronously; the listener is a QObject of its own so that its
        // connections end with this interface.
        m_listener = std::make_unique<QObject>();
        QObject::connect(view->text(), &TextLayer::textChanged, m_listener.get(),
                         [this](quint32 page) {
                             const auto it = m_pages.constFind(page);
                             if (it != m_pages.constEnd()) {
                                 (*it)->textArrived();
                             }
                         });
    }
    ~CanvasAccessible() override { purge(); }

    CanvasView* view() const { return qobject_cast<CanvasView*>(object()); }

    PageAccessible* pageInterface(quint32 page) {
        syncCount();
        if (view() == nullptr || page >= m_count) {
            return nullptr;
        }
        PageAccessible*& entry = m_pages[page];
        if (entry == nullptr) {
            entry = new PageAccessible(this, page);
            QAccessible::registerAccessibleInterface(entry);
        }
        return entry;
    }

    int childCount() const override {
        const CanvasView* v = view();
        return v == nullptr
                   ? 0
                   : static_cast<int>(const_cast<CanvasView*>(v)->controller()->pageCount());
    }
    QAccessibleInterface* child(int index) const override {
        if (index < 0) {
            return nullptr;
        }
        return const_cast<CanvasAccessible*>(this)->pageInterface(static_cast<quint32>(index));
    }
    int indexOfChild(const QAccessibleInterface* child) const override {
        for (auto it = m_pages.cbegin(); it != m_pages.cend(); ++it) {
            if (static_cast<const QAccessibleInterface*>(it.value()) == child) {
                return static_cast<int>(it.key());
            }
        }
        return -1;
    }
    QAccessibleInterface* childAt(int x, int y) const override {
        CanvasView* v = view();
        if (v == nullptr) {
            return nullptr;
        }
        const QPointF local = v->viewport()->mapFromGlobal(QPoint(x, y));
        if (const auto hit = v->pagePointAt(local)) {
            return const_cast<CanvasAccessible*>(this)->pageInterface(hit->page);
        }
        return nullptr;
    }
    QAccessibleInterface* focusChild() const override {
        CanvasView* v = view();
        if (v == nullptr || !v->focusedLink()) {
            return nullptr;
        }
        const CanvasView::LinkRef ref = *v->focusedLink();
        return canvasLinkInterface(v, ref.page, ref.index);
    }
    QString text(QAccessible::Text t) const override {
        const CanvasView* v = view();
        if (v == nullptr) {
            return {};
        }
        if (t == QAccessible::Name) {
            const QString name = v->accessibleName();
            return name.isEmpty() ? tr("Document") : name;
        }
        if (t == QAccessible::Description) {
            auto* mutableView = const_cast<CanvasView*>(v);
            const quint32 count = mutableView->controller()->pageCount();
            return count == 0 ? QString()
                              : v->describePage(mutableView->controller()->currentPage());
        }
        return QAccessibleWidget::text(t);
    }
    QAccessible::State state() const override {
        QAccessible::State s = QAccessibleWidget::state();
        s.readOnly = true;
        return s;
    }

private:
    // Drops the cached pages when the document changed under them.
    void syncCount() {
        const CanvasView* v = view();
        const quint32 count =
            v == nullptr ? 0 : const_cast<CanvasView*>(v)->controller()->pageCount();
        if (count != m_count) {
            purge();
            m_count = count;
        }
    }
    void purge() {
        const auto pages = std::exchange(m_pages, {});
        for (PageAccessible* page : pages) {
            QAccessible::deleteAccessibleInterface(QAccessible::uniqueId(page));
        }
    }

    std::unique_ptr<QObject> m_listener;
    QHash<quint32, PageAccessible*> m_pages;
    quint32 m_count = 0;
};

// ---- link definitions ----

CanvasView* LinkAccessible::view() const {
    return m_canvas->view();
}

std::optional<Link> LinkAccessible::link() const {
    CanvasView* v = view();
    if (v == nullptr) {
        return std::nullopt;
    }
    const QList<Link> links = v->links()->linksOf(m_page);
    if (m_index < 0 || m_index >= links.size()) {
        return std::nullopt;
    }
    return links.at(m_index);
}

bool LinkAccessible::isValid() const {
    return link().has_value();
}

QAccessibleInterface* LinkAccessible::parent() const {
    return m_pageInterface;
}

QString LinkAccessible::text(QAccessible::Text t) const {
    const auto l = link();
    if (!l || t != QAccessible::Name) {
        return {};
    }
    return view()->describeLink(*l);
}

QRect LinkAccessible::rect() const {
    const auto l = link();
    CanvasView* v = view();
    if (!l || v == nullptr) {
        return {};
    }
    const QRectF local = v->controller()->drawFor(m_page).map(l->rect);
    return QRect(v->viewport()->mapToGlobal(local.toAlignedRect().topLeft()),
                 local.toAlignedRect().size());
}

QAccessible::State LinkAccessible::state() const {
    QAccessible::State s;
    s.focusable = true;
    s.linked = true;
    s.readOnly = true;
    CanvasView* v = view();
    if (v == nullptr) {
        s.invalid = true;
        return s;
    }
    s.focused = v->focusedLink() == CanvasView::LinkRef{m_page, m_index};
    const QRect global = rect();
    const QRect local(v->viewport()->mapFromGlobal(global.topLeft()), global.size());
    s.offscreen = !v->viewport()->rect().intersects(local);
    return s;
}

void LinkAccessible::doAction(const QString& name) {
    CanvasView* v = view();
    if (v == nullptr) {
        return;
    }
    if (name == QAccessibleActionInterface::setFocusAction()) {
        v->focusLink(m_page, m_index);
    } else if (name == QAccessibleActionInterface::pressAction()) {
        if (v->focusLink(m_page, m_index)) {
            v->activateFocusedLink();
        }
    }
}

// ---- page definitions ----

CanvasView* PageAccessible::view() const {
    return m_canvas->view();
}

bool PageAccessible::isValid() const {
    CanvasView* v = view();
    return v != nullptr && m_page < v->controller()->pageCount();
}

QAccessibleInterface* PageAccessible::parent() const {
    return m_canvas;
}

LinkAccessible* PageAccessible::linkInterface(int index) {
    CanvasView* v = view();
    if (v == nullptr || index < 0 || index >= v->links()->linksOf(m_page).size()) {
        return nullptr;
    }
    LinkAccessible*& entry = m_links[index];
    if (entry == nullptr) {
        entry = new LinkAccessible(m_canvas, this, m_page, index);
        QAccessible::registerAccessibleInterface(entry);
    }
    return entry;
}

int PageAccessible::childCount() const {
    CanvasView* v = view();
    if (v == nullptr) {
        return 0;
    }
    // The links of a page are asked for the first time a reader looks.
    v->links()->ensure(m_page);
    return static_cast<int>(v->links()->linksOf(m_page).size());
}

int PageAccessible::indexOfChild(const QAccessibleInterface* child) const {
    for (const LinkAccessible* link : m_links) {
        if (static_cast<const QAccessibleInterface*>(link) == child) {
            return link->index();
        }
    }
    return -1;
}

QAccessibleInterface* PageAccessible::childAt(int x, int y) const {
    // The topmost link is the last one listed.
    PageAccessible* self = const_cast<PageAccessible*>(this);
    for (int i = childCount() - 1; i >= 0; --i) {
        QAccessibleInterface* link = self->linkInterface(i);
        if (link != nullptr && link->rect().contains(x, y)) {
            return link;
        }
    }
    return nullptr;
}

QString PageAccessible::text(QAccessible::Text t) const {
    CanvasView* v = view();
    if (v == nullptr || t != QAccessible::Name) {
        return {};
    }
    return v->describePage(m_page);
}

QRect PageAccessible::toGlobal(const QRectF& viewportRect) const {
    CanvasView* v = view();
    if (v == nullptr) {
        return {};
    }
    const QRect aligned = viewportRect.toAlignedRect();
    return QRect(v->viewport()->mapToGlobal(aligned.topLeft()), aligned.size());
}

QRect PageAccessible::rect() const {
    CanvasView* v = view();
    return v == nullptr ? QRect() : toGlobal(v->controller()->drawFor(m_page).rect);
}

QAccessible::State PageAccessible::state() const {
    QAccessible::State s;
    s.readOnly = true;
    s.selectableText = true;
    CanvasView* v = view();
    if (v == nullptr) {
        s.invalid = true;
        return s;
    }
    s.offscreen =
        !v->viewport()->rect().intersects(v->controller()->drawFor(m_page).rect.toAlignedRect());
    return s;
}

const PageString& PageAccessible::content() const {
    CanvasView* v = view();
    const PageText* text = v == nullptr ? nullptr : v->text()->page(m_page);
    if (text == nullptr) {
        if (v != nullptr) {
            v->text()->ensure(m_page);
        }
        return m_content;
    }
    if (m_contentChars != text->chars.size()) {
        m_content = pageString(*text);
        m_contentChars = text->chars.size();
    }
    return m_content;
}

void PageAccessible::textArrived() {
    if (!QAccessible::isActive()) {
        return;
    }
    const int before = static_cast<int>(m_content.text.size());
    const PageString& now = content();
    if (now.text.size() > before) {
        QAccessibleTextInsertEvent event(this, before, now.text.mid(before));
        QAccessible::updateAccessibility(&event);
    }
}

int PageAccessible::characterAt(int offset) const {
    const PageString& c = content();
    if (offset < 0 || offset >= c.text.size()) {
        return -1;
    }
    const auto it = std::upper_bound(c.starts.cbegin(), c.starts.cend(), offset);
    return static_cast<int>(it - c.starts.cbegin()) - 1;
}

QString PageAccessible::text(int startOffset, int endOffset) const {
    const QString& all = content().text;
    const int start = std::clamp(startOffset, 0, static_cast<int>(all.size()));
    const int end = endOffset < 0 ? static_cast<int>(all.size())
                                  : std::clamp(endOffset, start, static_cast<int>(all.size()));
    return all.mid(start, end - start);
}

QRect PageAccessible::characterRect(int offset) const {
    CanvasView* v = view();
    const int index = characterAt(offset);
    if (v == nullptr || index < 0) {
        return {};
    }
    const PageText* text = v->text()->page(m_page);
    if (text == nullptr || index >= text->chars.size() || text->chars.at(index).rect.isEmpty()) {
        return {};
    }
    return toGlobal(v->controller()->drawFor(m_page).map(text->chars.at(index).rect));
}

int PageAccessible::offsetAtPoint(const QPoint& point) const {
    CanvasView* v = view();
    if (v == nullptr) {
        return -1;
    }
    const auto hit = v->pagePointAt(v->viewport()->mapFromGlobal(point));
    const PageText* text = v->text()->page(m_page);
    if (!hit || hit->page != m_page || text == nullptr) {
        return -1;
    }
    const PageString& c = content();
    const quint32 caret = text->caretAt(hit->point);
    return caret < static_cast<quint32>(c.starts.size()) ? c.starts.at(static_cast<int>(caret))
                                                         : -1;
}

void PageAccessible::selection(int selectionIndex, int* startOffset, int* endOffset) const {
    *startOffset = 0;
    *endOffset = 0;
    CanvasView* v = view();
    quint32 from = 0;
    quint32 to = 0;
    if (selectionIndex != 0 || v == nullptr || !v->selection()->rangeOn(m_page, &from, &to)) {
        return;
    }
    const PageString& c = content();
    const auto offsetOf = [&](quint32 caret) {
        return caret >= static_cast<quint32>(c.starts.size())
                   ? static_cast<int>(c.text.size())
                   : c.starts.at(static_cast<int>(caret));
    };
    *startOffset = offsetOf(from);
    *endOffset = offsetOf(to);
}

int PageAccessible::cursorPosition() const {
    CanvasView* v = view();
    const auto caret = v == nullptr ? std::nullopt : v->caret()->position();
    if (!caret || caret->page != m_page) {
        return 0;
    }
    const PageString& c = content();
    return caret->caret >= static_cast<quint32>(c.starts.size())
               ? static_cast<int>(c.text.size())
               : c.starts.at(static_cast<int>(caret->caret));
}

void PageAccessible::setCursorPosition(int position) {
    CanvasView* v = view();
    const PageText* text = v == nullptr ? nullptr : v->text()->page(m_page);
    if (v == nullptr || text == nullptr || !v->caret()->isOn()) {
        return;
    }
    const int index = std::max(characterAt(position), 0);
    // A caret is placed by a point, so aim at the left edge of the character.
    if (index < text->chars.size() && text->chars.at(index).hasBox()) {
        const QRectF& box = text->chars.at(index).rect;
        v->caret()->placeAt(m_page, QPointF(box.left() + box.width() / 4.0, box.center().y()));
    }
}

int PageAccessible::selectionCount() const {
    CanvasView* v = view();
    quint32 from = 0;
    quint32 to = 0;
    return v != nullptr && v->selection()->rangeOn(m_page, &from, &to) ? 1 : 0;
}

void PageAccessible::scrollToSubstring(int startIndex, int endIndex) {
    CanvasView* v = view();
    const int first = characterAt(startIndex);
    const int last = characterAt(std::max(startIndex, endIndex - 1));
    const PageText* text = v == nullptr ? nullptr : v->text()->page(m_page);
    if (text == nullptr || first < 0 || last < first) {
        return;
    }
    QRectF area;
    for (int i = first; i <= last && i < text->chars.size(); ++i) {
        area = area.united(text->chars.at(i).rect);
    }
    if (!area.isEmpty()) {
        v->controller()->revealRect(m_page, area);
    }
}

QString PageAccessible::attributes(int, int* startOffset, int* endOffset) const {
    *startOffset = 0;
    *endOffset = characterCount();
    return {};
}

QAccessibleInterface* factory(const QString& className, QObject* object) {
    if (className == QLatin1String("vellora::CanvasView")) {
        if (auto* view = qobject_cast<CanvasView*>(object)) {
            return new CanvasAccessible(view);
        }
    }
    return nullptr;
}

} // namespace

void installCanvasAccessibility() {
    static const bool installed = [] {
        QAccessible::installFactory(&factory);
        return true;
    }();
    Q_UNUSED(installed);
}

QAccessibleInterface* canvasPageInterface(CanvasView* view, quint32 page) {
    auto* canvas = dynamic_cast<CanvasAccessible*>(QAccessible::queryAccessibleInterface(view));
    return canvas == nullptr ? nullptr : canvas->pageInterface(page);
}

QAccessibleInterface* canvasLinkInterface(CanvasView* view, quint32 page, int index) {
    auto* canvas = dynamic_cast<CanvasAccessible*>(QAccessible::queryAccessibleInterface(view));
    if (canvas == nullptr) {
        return nullptr;
    }
    PageAccessible* pageInterface = canvas->pageInterface(page);
    return pageInterface == nullptr ? nullptr : pageInterface->linkInterface(index);
}

} // namespace vellora
