#include "CrashDialog.h"

#include <QHBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QVBoxLayout>

namespace vellora {

CrashDialog::CrashDialog(const QList<CrashReports::Report>& reports, QWidget* parent)
    : QDialog(parent) {
    setObjectName(QStringLiteral("crashDialog"));
    setWindowTitle(tr("Crash reports"));

    qsizetype application = 0;
    for (const CrashReports::Report& report : reports) {
        application += report.application ? 1 : 0;
    }
    const qsizetype engine = reports.size() - application;
    QString text = application > 0 ? tr("Vellora closed unexpectedly the last time it ran.")
                                   : tr("The page renderer stopped unexpectedly.");
    text += QLatin1String("\n\n");
    text += tr("%n crash report(s) were saved on this computer (%1 of the application, %2 of the "
               "page renderer). Nothing was sent anywhere.",
               nullptr, static_cast<int>(reports.size()))
                .arg(application)
                .arg(engine);
    text += QLatin1String("\n\n");
    text += tr("To report the problem, open the issue form: it is filled in with the version and "
               "your system, and no file is attached. You can attach files from the crash folder "
               "yourself. A .dmp file holds the stacks of the program's threads, not its memory "
               "as a whole, but check it before you share it: issues are public.");
    m_message = new QLabel(text, this);
    m_message->setTextFormat(Qt::PlainText);
    m_message->setWordWrap(true);

    m_openFolder = new QPushButton(tr("Open crash folder"), this);
    m_report = new QPushButton(tr("Report on GitHub"), this);
    m_dismiss = new QPushButton(tr("Dismiss"), this);
    m_dismiss->setDefault(true);
    connect(m_openFolder, &QPushButton::clicked, this, [this] { choose(Choice::OpenFolder); });
    connect(m_report, &QPushButton::clicked, this, [this] { choose(Choice::Report); });
    connect(m_dismiss, &QPushButton::clicked, this, [this] { choose(Choice::Dismiss); });

    auto* buttons = new QHBoxLayout;
    buttons->addStretch(1);
    buttons->addWidget(m_openFolder);
    buttons->addWidget(m_report);
    buttons->addWidget(m_dismiss);
    auto* layout = new QVBoxLayout(this);
    layout->addWidget(m_message);
    layout->addLayout(buttons);
    setMinimumWidth(520);
}

void CrashDialog::choose(Choice choice) {
    if (m_choice == Choice::None) {
        m_choice = choice;
        emit chosen(choice);
    }
    accept();
}

void CrashDialog::done(int result) {
    // Closed with the window's button or Escape: the same as Dismiss.
    if (m_choice == Choice::None) {
        m_choice = Choice::Dismiss;
        emit chosen(m_choice);
    }
    QDialog::done(result);
}

} // namespace vellora