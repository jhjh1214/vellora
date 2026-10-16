// The part of printing that is about the document and not about the printer: which pages (all, the
// current one, or a typed range that may use the document's page labels), odd or even pages only,
// how a page is fitted to the paper, turning and centring, and grey. The printer, the number of
// copies and the paper come from the system's own print dialog afterwards.
#pragma once

#include "print/PrintRequest.h"

#include <QDialog>

class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
class QRadioButton;

namespace vellora {

class PrintOptionsDialog : public QDialog {
    Q_OBJECT

public:
    PrintOptionsDialog(const PrintContext& context, QWidget* parent = nullptr);

    // The pages chosen, in page order, once each; empty if the range typed is not understood
    // (`error` says why). Valid after `accept`.
    QList<quint32> pages(QString* error = nullptr) const;
    PrintOptions options() const;

    // The controls, for tests.
    QRadioButton* allPages() const { return m_all; }
    QRadioButton* currentPage() const { return m_current; }
    QRadioButton* pageRange() const { return m_range; }
    QLineEdit* rangeField() const { return m_rangeField; }
    QComboBox* parity() const { return m_parity; }
    QComboBox* scale() const { return m_scale; }
    QCheckBox* autoRotate() const { return m_rotate; }
    QCheckBox* center() const { return m_center; }
    QCheckBox* grayscale() const { return m_grayscale; }
    QLabel* problem() const { return m_problem; }

    void accept() override;

private:
    PrintContext m_context;
    QRadioButton* m_all = nullptr;
    QRadioButton* m_current = nullptr;
    QRadioButton* m_range = nullptr;
    QLineEdit* m_rangeField = nullptr;
    QComboBox* m_parity = nullptr;
    QComboBox* m_scale = nullptr;
    QCheckBox* m_rotate = nullptr;
    QCheckBox* m_center = nullptr;
    QCheckBox* m_grayscale = nullptr;
    QLabel* m_problem = nullptr;
};

} // namespace vellora
