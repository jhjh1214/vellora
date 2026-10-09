// The question asked before an address from a document is opened: the whole address, as plain
// text, and "do not ask again for this host" (for web addresses, for this document and this run
// only). The answer defaults to not opening.
#pragma once

#include <QDialog>

class QCheckBox;

namespace vellora {

class UriConfirmDialog : public QDialog {
    Q_OBJECT

public:
    // `host` is empty for an address without one (a mail address): then there is nothing to trust.
    UriConfirmDialog(const QString& uri, const QString& host, QWidget* parent = nullptr);

    // Whether the reader ticked "do not ask again for this host".
    bool trustHost() const;

private:
    QCheckBox* m_trust = nullptr;
};

} // namespace vellora
