#include "search/FindBar.h"

#include <QCheckBox>
#include <QEvent>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QLineEdit>
#include <QShortcut>
#include <QToolButton>

namespace vellora {

FindBar::FindBar(SearchController* search, QWidget* parent) : QWidget(parent), m_search(search) {
    m_edit = new QLineEdit(this);
    m_edit->setPlaceholderText(tr("Find"));
    m_edit->setClearButtonEnabled(true);
    m_edit->setAccessibleName(tr("Find"));
    m_edit->setMinimumWidth(220);
    // The text is searched for after a pause; Enter and Shift+Enter are handled below.
    m_edit->installEventFilter(this);

    m_case = new QCheckBox(tr("Match case"), this);
    m_words = new QCheckBox(tr("Whole words"), this);
    m_regex = new QCheckBox(tr("Regular expression"), this);
    m_regex->setToolTip(tr("Rust regular expression syntax (no look-around or back-references)"));

    m_previous = new QToolButton(this);
    m_previous->setText(QStringLiteral("▲"));
    m_previous->setToolTip(tr("Previous result (Shift+F3)"));
    m_previous->setAccessibleName(tr("Previous result"));
    m_next = new QToolButton(this);
    m_next->setText(QStringLiteral("▼"));
    m_next->setToolTip(tr("Next result (F3)"));
    m_next->setAccessibleName(tr("Next result"));
    m_results = new QToolButton(this);
    m_results->setText(tr("List"));
    m_results->setToolTip(tr("Show all results (Ctrl+Shift+F)"));
    m_results->setAccessibleName(tr("Show all results"));
    auto* close = new QToolButton(this);
    close->setText(QStringLiteral("✕"));
    close->setToolTip(tr("Close (Esc)"));
    close->setAccessibleName(tr("Close the find bar"));

    m_status = new QLabel(this);
    m_status->setTextFormat(Qt::PlainText); // an error comes from the engine
    m_status->setAccessibleName(tr("Search status"));
    m_status->setMinimumWidth(120);

    auto* layout = new QHBoxLayout(this);
    layout->setContentsMargins(6, 4, 6, 4);
    layout->addWidget(m_edit, 1);
    layout->addWidget(m_case);
    layout->addWidget(m_words);
    layout->addWidget(m_regex);
    layout->addWidget(m_previous);
    layout->addWidget(m_next);
    layout->addWidget(m_results);
    layout->addWidget(m_status);
    layout->addWidget(close);
    setFocusProxy(m_edit);
    setAutoFillBackground(true);
    hide();

    m_pause.setSingleShot(true);
    m_pause.setInterval(kDebounceMs);
    connect(&m_pause, &QTimer::timeout, this, &FindBar::searchNow);
    connect(m_edit, &QLineEdit::textChanged, this, [this] { queryChanged(); });
    for (QCheckBox* box : {m_case, m_words, m_regex}) {
        connect(box, &QCheckBox::toggled, this, [this] { queryChanged(); });
    }
    connect(m_next, &QToolButton::clicked, this, [this] {
        searchNow();
        m_search->next();
    });
    connect(m_previous, &QToolButton::clicked, this, [this] {
        searchNow();
        m_search->previous();
    });
    connect(m_results, &QToolButton::clicked, this, &FindBar::resultsRequested);
    connect(close, &QToolButton::clicked, this, &FindBar::closeBar);
    // Escape closes the bar wherever in it the focus is.
    auto* escape = new QShortcut(QKeySequence(Qt::Key_Escape), this);
    escape->setContext(Qt::WidgetWithChildrenShortcut);
    connect(escape, &QShortcut::activated, this, &FindBar::closeBar);
    connect(m_search, &SearchController::statusChanged, this, [this] { refreshStatus(); });
    connect(m_search, &SearchController::currentChanged, this, [this] { refreshStatus(); });
}

SearchController::Query FindBar::query() const {
    return {m_edit->text(), m_case->isChecked(), m_words->isChecked(), m_regex->isChecked()};
}

void FindBar::open(const QString& text) {
    show();
    if (!text.isEmpty()) {
        m_edit->setText(text);
        searchNow();
    }
    m_edit->setFocus(Qt::ShortcutFocusReason);
    m_edit->selectAll();
}

void FindBar::closeBar() {
    m_pause.stop();
    hide();
    m_search->clear();
    emit closed();
}

void FindBar::reset() {
    m_pause.stop();
    hide();
    m_edit->clear();
    m_search->clear();
}

void FindBar::queryChanged() {
    if (m_edit->text().isEmpty()) {
        m_pause.stop();
        m_search->clear();
        return;
    }
    m_pause.start();
}

void FindBar::searchNow() {
    m_pause.stop();
    const SearchController::Query wanted = query();
    // Stepping with Enter must not restart a search that is already running for this query.
    if (wanted == m_search->query() && m_search->state() != SearchController::State::Idle) {
        return;
    }
    m_search->start(wanted);
}

void FindBar::refreshStatus() {
    m_status->setText(m_search->statusText());
    m_status->setToolTip(m_search->state() == SearchController::State::Failed ? m_search->error()
                                                                              : QString());
}

bool FindBar::eventFilter(QObject* watched, QEvent* event) {
    if (watched == m_edit && event->type() == QEvent::KeyPress) {
        const auto* key = static_cast<QKeyEvent*>(event);
        if (key->key() == Qt::Key_Return || key->key() == Qt::Key_Enter) {
            searchNow();
            if (key->modifiers().testFlag(Qt::ShiftModifier)) {
                m_search->previous();
            } else {
                m_search->next();
            }
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
}

} // namespace vellora
