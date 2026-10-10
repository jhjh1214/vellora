#include "search/SearchResultsView.h"

#include <QApplication>
#include <QLabel>
#include <QPainter>
#include <QStyledItemDelegate>
#include <QVBoxLayout>

namespace vellora {

namespace {

// Two lines per hit: "Page 12", and the words around the match with the match in bold.
class HitDelegate : public QStyledItemDelegate {
public:
    using QStyledItemDelegate::QStyledItemDelegate;

    QSize sizeHint(const QStyleOptionViewItem& option, const QModelIndex&) const override {
        return {option.rect.width(), 2 * option.fontMetrics.height() + 10};
    }

    void paint(QPainter* painter, const QStyleOptionViewItem& option,
               const QModelIndex& index) const override {
        QStyleOptionViewItem plain = option;
        initStyleOption(&plain, index);
        plain.text.clear();
        const QWidget* widget = option.widget;
        QStyle* style = widget != nullptr ? widget->style() : QApplication::style();
        style->drawControl(QStyle::CE_ItemViewItem, &plain, painter, widget);

        const bool selected = option.state.testFlag(QStyle::State_Selected);
        const QPalette::ColorRole role = selected ? QPalette::HighlightedText : QPalette::Text;
        const QRect rect = option.rect.adjusted(6, 3, -6, -3);
        const QFontMetrics metrics = option.fontMetrics;

        painter->save();
        painter->setClipRect(option.rect);
        painter->setPen(
            option.palette.color(selected ? QPalette::Active : QPalette::Disabled, role));
        painter->drawText(
            rect.left(), rect.top() + metrics.ascent(),
            QObject::tr("Page %1").arg(index.data(SearchResultsModel::PageLabelRole).toString()));

        painter->setPen(option.palette.color(QPalette::Active, role));
        int x = rect.left();
        const int y = rect.top() + metrics.height() + metrics.ascent();
        QFont bold = option.font;
        bold.setBold(true);
        const QFontMetrics boldMetrics(bold);
        const auto draw = [&](const QString& text, const QFont& font, const QFontMetrics& fm) {
            // A hit is one line: clipped at the edge, not wrapped.
            painter->setFont(font);
            painter->drawText(x, y, text);
            x += fm.horizontalAdvance(text);
        };
        draw(index.data(SearchResultsModel::BeforeRole).toString(), option.font, metrics);
        draw(index.data(SearchResultsModel::MatchRole).toString(), bold, boldMetrics);
        draw(index.data(SearchResultsModel::AfterRole).toString(), option.font, metrics);
        painter->restore();
    }
};

} // namespace

SearchResultsModel::SearchResultsModel(SearchController* search, QObject* parent)
    : QAbstractListModel(parent), m_search(search) {
    m_labeler = [](quint32 page) { return QString::number(page + 1); };
    connect(m_search, &SearchController::cleared, this, [this] {
        beginResetModel();
        m_rows = 0;
        endResetModel();
    });
    connect(m_search, &SearchController::hitsAdded, this, [this](int first, int count) {
        // Rows are announced as they appear: the list keeps its place and its selection.
        if (first != m_rows || count <= 0) {
            return;
        }
        beginInsertRows(QModelIndex(), first, first + count - 1);
        m_rows = first + count;
        endInsertRows();
    });
}

void SearchResultsModel::setPageLabeler(PageLabeler labeler) {
    m_labeler = std::move(labeler);
    beginResetModel();
    endResetModel();
}

void SearchResultsModel::refreshLabels() {
    if (m_rows > 0) {
        emit dataChanged(index(0), index(m_rows - 1), {PageLabelRole, Qt::AccessibleTextRole});
    }
}

int SearchResultsModel::rowCount(const QModelIndex& parent) const {
    return parent.isValid() ? 0 : m_rows;
}

QVariant SearchResultsModel::data(const QModelIndex& index, int role) const {
    if (!index.isValid() || index.row() < 0 || index.row() >= m_rows) {
        return {};
    }
    const SearchHit& hit = m_search->hits().at(index.row());
    switch (role) {
    case Qt::DisplayRole:
        return hit.snippet;
    case PageLabelRole:
        return m_labeler(hit.page);
    case BeforeRole:
        return hit.snippet.left(hit.matchStart);
    case MatchRole:
        return hit.snippet.mid(hit.matchStart, hit.matchLength);
    case AfterRole:
        return hit.snippet.mid(hit.matchStart + hit.matchLength);
    case Qt::AccessibleTextRole:
        return tr("Page %1: %2").arg(m_labeler(hit.page), hit.snippet);
    default:
        return {};
    }
}

SearchResultsView::SearchResultsView(SearchController* search, QWidget* parent)
    : QWidget(parent), m_search(search), m_model(search, this) {
    m_summary = new QLabel(this);
    m_summary->setTextFormat(Qt::PlainText);
    m_summary->setMargin(6);
    m_summary->setWordWrap(true);
    m_summary->setAccessibleName(tr("Search summary"));
    m_list = new QListView(this);
    m_list->setModel(&m_model);
    m_list->setItemDelegate(new HitDelegate(m_list));
    m_list->setUniformItemSizes(true);
    m_list->setEditTriggers(QAbstractItemView::NoEditTriggers);
    m_list->setSelectionMode(QAbstractItemView::SingleSelection);
    m_list->setAccessibleName(tr("Search results"));
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(m_summary);
    layout->addWidget(m_list, 1);
    setFocusProxy(m_list);

    const auto refresh = [this] {
        m_summary->setText(m_search->state() == SearchController::State::Idle
                               ? tr("Press Ctrl+F to search the document.")
                               : m_search->statusText());
    };
    refresh();
    connect(m_search, &SearchController::statusChanged, this, refresh);
    connect(m_search, &SearchController::currentChanged, this, [this](int index) {
        if (index < 0) {
            m_list->clearSelection();
        } else {
            m_list->setCurrentIndex(m_model.index(index));
        }
    });
    connect(m_list, &QListView::clicked, this,
            [this](const QModelIndex& index) { emit hitChosen(index.row()); });
    connect(m_list, &QListView::activated, this,
            [this](const QModelIndex& index) { emit hitChosen(index.row()); });
}

} // namespace vellora
