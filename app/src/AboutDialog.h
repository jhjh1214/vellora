// Help -> About Vellora: what this build is, under which license it is given, the notice that Qt
// is used under the LGPL with how to replace it, and a way to read the third-party licenses. All
// texts are plain text.
#pragma once

#include <QDialog>

class QLabel;
class QPushButton;

namespace vellora {

class AboutDialog : public QDialog {
    Q_OBJECT

public:
    explicit AboutDialog(QWidget* parent = nullptr);

    // "Version 0.0.0, commit abc, built 2026-10-09" (from the build, see BuildInfo.h.in).
    QLabel* versionLabel() const { return m_version; }
    // The MPL-2.0 statement.
    QLabel* licenseLabel() const { return m_license; }
    // The Qt LGPL notice, with the folders Qt is loaded from and how to replace it.
    QLabel* qtLabel() const { return m_qt; }
    QPushButton* licenseButton() const { return m_licenseButton; }
    QPushButton* thirdPartyButton() const { return m_thirdPartyButton; }

    // Opens the license texts (non-blocking); the dialogs are children of this one.
    QDialog* showLicense();
    QDialog* showThirdPartyLicenses();

private:
    QLabel* m_version = nullptr;
    QLabel* m_license = nullptr;
    QLabel* m_qt = nullptr;
    QPushButton* m_licenseButton = nullptr;
    QPushButton* m_thirdPartyButton = nullptr;
};

} // namespace vellora