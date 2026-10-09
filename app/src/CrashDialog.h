// Shown at start-up when crash reports were written since the user last looked: the application
// closed unexpectedly, or the page renderer did. Three choices, none of which uploads anything:
// open the crash folder, open a prefilled GitHub issue (nothing attached), or dismiss.
#pragma once

#include "diagnostics/CrashReports.h"

#include <QDialog>

class QLabel;
class QPushButton;

namespace vellora {

class CrashDialog : public QDialog {
    Q_OBJECT

public:
    enum class Choice { None, OpenFolder, Report, Dismiss };

    CrashDialog(const QList<CrashReports::Report>& reports, QWidget* parent = nullptr);

    QLabel* message() const { return m_message; }
    QPushButton* openFolderButton() const { return m_openFolder; }
    QPushButton* reportButton() const { return m_report; }
    QPushButton* dismissButton() const { return m_dismiss; }

    // What the user chose; `Dismiss` also when the dialog is closed any other way.
    Choice choice() const { return m_choice; }

signals:
    // Emitted once, when the user has chosen (or closed the dialog).
    void chosen(CrashDialog::Choice choice);

protected:
    void done(int result) override;

private:
    void choose(Choice choice);

    QLabel* m_message = nullptr;
    QPushButton* m_openFolder = nullptr;
    QPushButton* m_report = nullptr;
    QPushButton* m_dismiss = nullptr;
    Choice m_choice = Choice::None;
};

} // namespace vellora