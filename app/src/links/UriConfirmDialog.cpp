#include "links/UriConfirmDialog.h"

#include <QCheckBox>
#include <QDialogButtonBox>
#include <QFontDatabase>
#include <QLabel>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

namespace vellora {

UriConfirmDialog::UriConfirmDialog(const QString& uri, const QString& host, QWidget* parent)
    : QDialog(parent) {
    setWindowTitle(tr("Open Link"));
    setModal(true);
    auto* layout = new QVBoxLayout(this);

    auto* question = new QLabel(tr("This document asks to open this address in another "
                                   "application. Open it only if you trust the document."),
                                this);
    question->setWordWrap(true);
    question->setTextFormat(Qt::PlainText);
    layout->addWidget(question);

    // A read-only text box: the address can be long, and is shown whole and as written.
    auto* address = new QPlainTextEdit(this);
    address->setReadOnly(true);
    address->setPlainText(uri);
    address->setLineWrapMode(QPlainTextEdit::WidgetWidth);
    address->setWordWrapMode(QTextOption::WrapAnywhere);
    address->setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    address->setAccessibleName(tr("Address"));
    address->setMinimumHeight(address->fontMetrics().lineSpacing() * 4);
    layout->addWidget(address);

    if (!host.isEmpty()) {
        QString shown = host;
        m_trust = new QCheckBox(tr("Do not ask again for %1 in this document")
                                    .arg(shown.replace(QLatin1Char('&'), QStringLiteral("&&"))),
                                this);
        layout->addWidget(m_trust);
    }

    auto* buttons = new QDialogButtonBox(this);
    QPushButton* open = buttons->addButton(tr("Open"), QDialogButtonBox::AcceptRole);
    QPushButton* cancel = buttons->addButton(QDialogButtonBox::Cancel);
    cancel->setDefault(true);
    cancel->setFocus();
    Q_UNUSED(open);
    connect(buttons, &QDialogButtonBox::accepted, this, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    layout->addWidget(buttons);
    resize(480, sizeHint().height());
}

bool UriConfirmDialog::trustHost() const {
    return m_trust != nullptr && m_trust->isChecked();
}

} // namespace vellora
