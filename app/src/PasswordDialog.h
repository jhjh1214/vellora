// The modal prompt for a document password. It shows the typed text as dots and never keeps it:
// the caller takes the text once, and the field is cleared when the dialog closes. The file name
// and every message are shown as plain text, because they are untrusted.
#pragma once

#include <QDialog>
#include <QString>

class QLabel;
class QLineEdit;

namespace vellora {

class PasswordDialog : public QDialog {
    Q_OBJECT

public:
    // `attempt` counts from 1 of `maxAttempts`; `wrong` says the previous password was refused.
    PasswordDialog(const QString& fileName, int attempt, int maxAttempts, bool wrong,
                   QWidget* parent = nullptr);
    ~PasswordDialog() override;

    // The text typed so far; the field is emptied, so a second call returns an empty string.
    QString takePassword();
    QLineEdit* field() const { return m_field; }
    QLabel* message() const { return m_message; }

private:
    QLabel* m_message = nullptr;
    QLineEdit* m_field = nullptr;
};

} // namespace vellora