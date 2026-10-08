#include "PasswordDialog.h"

#include <QDialogButtonBox>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QVBoxLayout>

namespace vellora {

PasswordDialog::PasswordDialog(const QString& fileName, int attempt, int maxAttempts, bool wrong,
                               QWidget* parent)
    : QDialog(parent) {
    setObjectName(QStringLiteral("passwordDialog"));
    setWindowTitle(tr("Password required"));
    setModal(true);

    QString text = tr("“%1” is protected. Enter the password to open it.").arg(fileName);
    if (wrong) {
        text = tr("That password is not correct (attempt %1 of %2).\n\n%3")
                   .arg(attempt)
                   .arg(maxAttempts)
                   .arg(text);
    }
    m_message = new QLabel(text, this);
    m_message->setTextFormat(Qt::PlainText);
    m_message->setWordWrap(true);

    m_field = new QLineEdit(this);
    m_field->setEchoMode(QLineEdit::Password);
    // Not offered back to the user from a history, and not predicted from earlier input.
    m_field->setObjectName(QStringLiteral("passwordField"));
    m_field->setClearButtonEnabled(true);
    m_field->setInputMethodHints(Qt::ImhHiddenText | Qt::ImhNoPredictiveText |
                                 Qt::ImhNoAutoUppercase | Qt::ImhSensitiveData);

    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    buttons->button(QDialogButtonBox::Ok)->setText(tr("Open"));
    connect(buttons, &QDialogButtonBox::accepted, this, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);

    auto* layout = new QVBoxLayout(this);
    layout->addWidget(m_message);
    layout->addWidget(m_field);
    layout->addWidget(buttons);
    setMinimumWidth(380);
    m_field->setFocus();
}

PasswordDialog::~PasswordDialog() {
    m_field->clear();
}

QString PasswordDialog::takePassword() {
    const QString password = m_field->text();
    m_field->clear();
    return password;
}

} // namespace vellora