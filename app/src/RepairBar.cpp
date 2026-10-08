#include "RepairBar.h"

#include <QDialog>
#include <QDialogButtonBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

namespace vellora {

RepairBar::RepairBar(QWidget* parent) : QFrame(parent) {
    setObjectName(QStringLiteral("repairBar"));
    setFrameShape(QFrame::StyledPanel);
    setAutoFillBackground(true);
    // The palette's tooltip colours are the platform's "notice" colours in light and dark themes.
    QPalette notice = palette();
    notice.setColor(QPalette::Window, notice.color(QPalette::ToolTipBase));
    notice.setColor(QPalette::WindowText, notice.color(QPalette::ToolTipText));
    setPalette(notice);

    m_label = new QLabel(tr("This file was damaged and has been repaired for viewing."), this);
    m_label->setTextFormat(Qt::PlainText);
    m_label->setWordWrap(true);
    m_details = new QPushButton(tr("Details…"), this);
    m_dismiss = new QPushButton(tr("Dismiss"), this);

    auto* layout = new QHBoxLayout(this);
    layout->setContentsMargins(8, 4, 8, 4);
    layout->addWidget(m_label, 1);
    layout->addWidget(m_details);
    layout->addWidget(m_dismiss);

    connect(m_details, &QPushButton::clicked, this, [this] { showDetails(); });
    connect(m_dismiss, &QPushButton::clicked, this, &QWidget::hide);
    hide();
}

void RepairBar::setReasons(const QStringList& reasons) {
    delete m_dialog;
    m_dialog = nullptr;
    m_reasons = reasons;
    setVisible(!reasons.isEmpty());
}

QString RepairBar::text() const {
    return m_label->text();
}

QDialog* RepairBar::showDetails() {
    if (m_reasons.isEmpty()) {
        return nullptr;
    }
    if (!m_dialog) {
        m_dialog = new QDialog(this);
        m_dialog->setWindowTitle(tr("Repairs made to this file"));
        auto* layout = new QVBoxLayout(m_dialog);
        auto* intro = new QLabel(
            tr("Vellora read this file with repairs. The original file on disk is not changed."),
            m_dialog);
        intro->setTextFormat(Qt::PlainText);
        intro->setWordWrap(true);
        layout->addWidget(intro);
        auto* list = new QPlainTextEdit(m_dialog);
        list->setReadOnly(true);
        list->setPlainText(m_reasons.join(QLatin1Char('\n')));
        layout->addWidget(list, 1);
        auto* buttons = new QDialogButtonBox(QDialogButtonBox::Close, m_dialog);
        connect(buttons, &QDialogButtonBox::rejected, m_dialog, &QDialog::close);
        layout->addWidget(buttons);
        m_dialog->resize(480, 300);
    }
    m_dialog->open();
    return m_dialog;
}

} // namespace vellora
