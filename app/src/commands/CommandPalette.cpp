#include "commands/CommandPalette.h"

#include <QKeyEvent>
#include <QLineEdit>
#include <QListWidget>
#include <QVBoxLayout>

namespace vellora {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kWidth = 480;
constexpr int kVisibleRows = 8;

// Every whitespace-separated word of `filter` occurs in `title`, ignoring case.
bool matches(const QString& title, const QString& filter) {
    const QStringList words = filter.split(QLatin1Char(' '), Qt::SkipEmptyParts);
    for (const QString& word : words) {
        if (!title.contains(word, Qt::CaseInsensitive)) {
            return false;
        }
    }
    return true;
}

} // namespace

CommandPalette::CommandPalette(const CommandRegistry* registry, QWidget* parent)
    : QDialog(parent, Qt::Popup | Qt::FramelessWindowHint), m_registry(registry),
      m_search(new QLineEdit(this)), m_list(new QListWidget(this)) {
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(6, 6, 6, 6);
    m_search->setPlaceholderText(tr("Type a command"));
    layout->addWidget(m_search);
    m_list->setFocusPolicy(Qt::NoFocus);
    layout->addWidget(m_list);
    setFixedWidth(kWidth);

    connect(m_search, &QLineEdit::textChanged, this, &CommandPalette::refresh);
    connect(m_search, &QLineEdit::returnPressed, this, &CommandPalette::runCurrent);
    connect(m_list, &QListWidget::itemActivated, this, [this] { runCurrent(); });
    // Up and Down move the choice while the focus stays in the search box.
    m_search->installEventFilter(this);
}

void CommandPalette::open() {
    m_search->clear();
    refresh(QString());
    adjustSize();
    if (QWidget* window = parentWidget() ? parentWidget()->window() : nullptr) {
        const QPoint top = window->mapToGlobal(QPoint((window->width() - width()) / 2, 40));
        move(top);
    }
    show();
    m_search->setFocus();
}

void CommandPalette::refresh(const QString& filter) {
    m_list->clear();
    for (const Command& command : m_registry->commands()) {
        if (!matches(command.title, filter)) {
            continue;
        }
        QString text = command.title;
        if (!command.shortcuts.isEmpty()) {
            text += QStringLiteral("    ") +
                    command.shortcuts.first().toString(QKeySequence::NativeText);
        }
        // Item text is always plain text; titles and shortcuts are never read as markup.
        auto* item = new QListWidgetItem(text, m_list);
        item->setData(kIdRole, command.id);
    }
    if (m_list->count() > 0) {
        m_list->setCurrentRow(0);
    }
    const int rows = std::min(std::max(m_list->count(), 1), kVisibleRows);
    m_list->setFixedHeight(rows * m_list->sizeHintForRow(0) + 2 * m_list->frameWidth());
    adjustSize();
}

QStringList CommandPalette::visibleCommandIds() const {
    QStringList ids;
    for (int row = 0; row < m_list->count(); ++row) {
        ids.append(m_list->item(row)->data(kIdRole).toString());
    }
    return ids;
}

QString CommandPalette::currentCommandId() const {
    const QListWidgetItem* item = m_list->currentItem();
    return item != nullptr ? item->data(kIdRole).toString() : QString();
}

bool CommandPalette::runCurrent() {
    const QString id = currentCommandId();
    if (id.isEmpty()) {
        return false;
    }
    // Closed first: a command may open a dialog of its own.
    hide();
    return m_registry->run(id);
}

bool CommandPalette::eventFilter(QObject* watched, QEvent* event) {
    if (watched == m_search && event->type() == QEvent::KeyPress) {
        const int key = static_cast<QKeyEvent*>(event)->key();
        if (key == Qt::Key_Down || key == Qt::Key_Up) {
            const int count = m_list->count();
            if (count > 0) {
                const int step = key == Qt::Key_Down ? 1 : -1;
                m_list->setCurrentRow((m_list->currentRow() + step + count) % count);
            }
            return true;
        }
    }
    return QDialog::eventFilter(watched, event);
}

} // namespace vellora
