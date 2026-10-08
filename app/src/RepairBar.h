// A non-modal bar above the canvas that says the open file was damaged and has been repaired for
// viewing. "Details" lists the engine's reasons; "Dismiss" hides the bar until the next document.
// Everything shown comes from the engine, so it is all plain text.
#pragma once

#include <QFrame>
#include <QStringList>

class QDialog;
class QLabel;
class QPushButton;

namespace vellora {

class RepairBar : public QFrame {
    Q_OBJECT

public:
    explicit RepairBar(QWidget* parent = nullptr);

    // Shows the bar for these reasons, or hides it when there are none. Closes an open details
    // dialog of the previous document.
    void setReasons(const QStringList& reasons);
    QStringList reasons() const { return m_reasons; }
    // The one line the bar shows.
    QString text() const;

    // Opens the dialog that lists the reasons (non-blocking); null while there are none.
    QDialog* showDetails();
    QPushButton* detailsButton() const { return m_details; }
    QPushButton* dismissButton() const { return m_dismiss; }

private:
    QLabel* m_label = nullptr;
    QPushButton* m_details = nullptr;
    QPushButton* m_dismiss = nullptr;
    QDialog* m_dialog = nullptr;
    QStringList m_reasons;
};

} // namespace vellora
