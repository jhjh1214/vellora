#include "AboutDialog.h"

#include "BuildInfo.h"
#include "LicensesDialog.h"

#include <QCoreApplication>
#include <QDialogButtonBox>
#include <QFile>
#include <QLabel>
#include <QLibraryInfo>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>

namespace vellora {

namespace {

QLabel* textLabel(const QString& text, QWidget* parent) {
    auto* label = new QLabel(text, parent);
    label->setTextFormat(Qt::PlainText);
    label->setWordWrap(true);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    return label;
}

} // namespace

AboutDialog::AboutDialog(QWidget* parent) : QDialog(parent) {
    setObjectName(QStringLiteral("aboutDialog"));
    setWindowTitle(tr("About Vellora"));

    auto* title = new QLabel(tr("Vellora"), this);
    title->setTextFormat(Qt::PlainText);
    QFont big = title->font();
    big.setPointSizeF(big.pointSizeF() * 1.8);
    big.setBold(true);
    title->setFont(big);

    m_version =
        textLabel(tr("Version %1, commit %2, built %3")
                      .arg(QLatin1String(buildinfo::kVersion), QLatin1String(buildinfo::kCommit),
                           QLatin1String(buildinfo::kBuildDate)),
                  this);
    auto* tagline = textLabel(tr("A local-first, open-source PDF workstation. Your documents "
                                 "stay on this computer: Vellora sends nothing anywhere."),
                              this);

    m_license = textLabel(
        tr("Vellora is free software, licensed under the Mozilla Public License, version 2.0 "
           "(MPL-2.0). You may use, study, change and share it under that license; its text and "
           "the source code are available from the project."),
        this);
    m_licenseButton = new QPushButton(tr("License text…"), this);

    const QString applicationFolder = QCoreApplication::applicationDirPath();
    const QString qtFolder = QLibraryInfo::path(QLibraryInfo::LibrariesPath);
    m_qt = textLabel(
        tr("This program uses Qt %1 (built with Qt %2) under the GNU Lesser General Public "
           "License, version 3 (LGPL-3.0). Qt is linked dynamically, so you can replace it: put "
           "your own build of the same Qt version series (6.8) in place of the Qt libraries that "
           "Vellora loads, from the application folder (%3) or from Qt's library folder (%4). "
           "Qt's source code and license texts are available at https://code.qt.io/ and "
           "https://www.qt.io/licensing.")
            .arg(QLatin1String(qVersion()), QLatin1String(QT_VERSION_STR), applicationFolder,
                 qtFolder),
        this);
    auto* components = textLabel(
        tr("Pages are drawn by PDFium in a separate, sandboxed process. The licenses of every "
           "bundled component, including PDFium and the libraries inside it, are listed under "
           "Third-party licenses."),
        this);
    m_thirdPartyButton = new QPushButton(tr("Third-party licenses…"), this);

    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Close, this);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    connect(m_licenseButton, &QPushButton::clicked, this, [this] { showLicense(); });
    connect(m_thirdPartyButton, &QPushButton::clicked, this, [this] { showThirdPartyLicenses(); });

    auto* layout = new QVBoxLayout(this);
    layout->addWidget(title);
    layout->addWidget(m_version);
    layout->addWidget(tagline);
    layout->addSpacing(8);
    layout->addWidget(m_license);
    layout->addWidget(m_licenseButton, 0, Qt::AlignLeft);
    layout->addSpacing(8);
    layout->addWidget(m_qt);
    layout->addWidget(components);
    layout->addWidget(m_thirdPartyButton, 0, Qt::AlignLeft);
    layout->addWidget(buttons);
    setMinimumWidth(520);
}

QDialog* AboutDialog::showLicense() {
    auto* dialog = new QDialog(this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->setObjectName(QStringLiteral("licenseDialog"));
    dialog->setWindowTitle(tr("Vellora license"));
    auto* layout = new QVBoxLayout(dialog);
    auto* text = new QPlainTextEdit(dialog);
    text->setReadOnly(true);
    QFile file(QStringLiteral(":/licenses/LICENSE"));
    text->setPlainText(file.open(QIODevice::ReadOnly)
                           ? QString::fromUtf8(file.readAll())
                           : tr("The license text is missing from this build."));
    layout->addWidget(text);
    dialog->resize(640, 480);
    dialog->open();
    return dialog;
}

QDialog* AboutDialog::showThirdPartyLicenses() {
    auto* dialog = new LicensesDialog(this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->open();
    return dialog;
}

} // namespace vellora