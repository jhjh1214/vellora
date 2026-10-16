#include "print/PrintOptionsDialog.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QGroupBox>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QRadioButton>
#include <QVBoxLayout>
#include <algorithm>

namespace vellora {

PrintOptionsDialog::PrintOptionsDialog(const PrintContext& context, QWidget* parent)
    : QDialog(parent), m_context(context) {
    setObjectName(QStringLiteral("printOptionsDialog"));
    setWindowTitle(context.preview ? tr("Print Preview") : tr("Print"));
    setModal(true);

    auto* pagesBox = new QGroupBox(tr("Pages"), this);
    m_all = new QRadioButton(tr("All (%1)").arg(context.pageCount), pagesBox);
    m_current = new QRadioButton(tr("Current page"), pagesBox);
    m_range = new QRadioButton(tr("Pages:"), pagesBox);
    m_all->setChecked(true);
    m_rangeField = new QLineEdit(context.defaultRange, pagesBox);
    m_rangeField->setObjectName(QStringLiteral("printRangeField"));
    m_rangeField->setPlaceholderText(tr("for example 1-3, 7 or iv-vi"));
    m_rangeField->setAccessibleName(tr("Pages to print"));
    m_rangeField->setEnabled(false);
    connect(m_range, &QRadioButton::toggled, m_rangeField, &QWidget::setEnabled);
    connect(m_range, &QRadioButton::toggled, this, [this](bool on) {
        if (on) {
            m_rangeField->setFocus();
        }
    });
    m_parity = new QComboBox(pagesBox);
    m_parity->addItem(tr("All pages in the range"), static_cast<int>(PageParity::All));
    m_parity->addItem(tr("Odd pages only"), static_cast<int>(PageParity::Odd));
    m_parity->addItem(tr("Even pages only"), static_cast<int>(PageParity::Even));
    m_parity->setAccessibleName(tr("Odd or even pages"));
    auto* pagesLayout = new QVBoxLayout(pagesBox);
    pagesLayout->addWidget(m_all);
    pagesLayout->addWidget(m_current);
    pagesLayout->addWidget(m_range);
    pagesLayout->addWidget(m_rangeField);
    pagesLayout->addWidget(m_parity);

    auto* fitBox = new QGroupBox(tr("Page size"), this);
    m_scale = new QComboBox(fitBox);
    m_scale->addItem(tr("Fit to page"), static_cast<int>(PrintScale::FitToPage));
    m_scale->addItem(tr("Actual size"), static_cast<int>(PrintScale::ActualSize));
    m_scale->addItem(tr("Shrink oversized pages"), static_cast<int>(PrintScale::ShrinkOversized));
    m_scale->setAccessibleName(tr("How a page is fitted to the paper"));
    m_rotate = new QCheckBox(tr("Turn pages to match the paper"), fitBox);
    m_rotate->setChecked(true);
    m_center = new QCheckBox(tr("Centre on the paper"), fitBox);
    m_center->setChecked(true);
    m_grayscale = new QCheckBox(tr("Print in grayscale"), fitBox);
    auto* fitLayout = new QVBoxLayout(fitBox);
    fitLayout->addWidget(m_scale);
    fitLayout->addWidget(m_rotate);
    fitLayout->addWidget(m_center);
    fitLayout->addWidget(m_grayscale);

    m_problem = new QLabel(this);
    m_problem->setObjectName(QStringLiteral("printProblem"));
    m_problem->setTextFormat(Qt::PlainText);
    m_problem->setWordWrap(true);
    m_problem->setAccessibleName(tr("Problem with the pages"));
    m_problem->hide();

    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
    buttons->button(QDialogButtonBox::Ok)->setText(context.preview ? tr("Preview") : tr("Next…"));
    connect(buttons, &QDialogButtonBox::accepted, this, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);

    auto* layout = new QVBoxLayout(this);
    layout->addWidget(pagesBox);
    layout->addWidget(fitBox);
    layout->addWidget(m_problem);
    layout->addWidget(buttons);
    setMinimumWidth(380);
}

QList<quint32> PrintOptionsDialog::pages(QString* error) const {
    QList<quint32> chosen;
    if (m_context.pageCount == 0) {
        if (error != nullptr) {
            *error = tr("There are no pages to print.");
        }
        return chosen;
    }
    if (m_current->isChecked()) {
        chosen.append(std::min(m_context.currentPage, m_context.pageCount - 1));
    } else if (m_range->isChecked()) {
        const PageRangeResult parsed = parsePageRanges(m_rangeField->text(), m_context.resolve);
        if (!parsed.ok()) {
            if (error != nullptr) {
                *error = parsed.error;
            }
            return {};
        }
        chosen = parsed.pages;
        std::sort(chosen.begin(), chosen.end());
    } else {
        for (quint32 page = 0; page < m_context.pageCount; ++page) {
            chosen.append(page);
        }
    }
    chosen = filterParity(chosen, static_cast<PageParity>(m_parity->currentData().toInt()));
    if (chosen.isEmpty() && error != nullptr) {
        *error = tr("None of those pages is an odd (or even) page.");
    }
    return chosen;
}

PrintOptions PrintOptionsDialog::options() const {
    PrintOptions options;
    options.scale = static_cast<PrintScale>(m_scale->currentData().toInt());
    options.autoRotate = m_rotate->isChecked();
    options.center = m_center->isChecked();
    options.grayscale = m_grayscale->isChecked();
    return options;
}

void PrintOptionsDialog::accept() {
    QString error;
    if (pages(&error).isEmpty()) {
        m_problem->setText(error);
        m_problem->show();
        if (m_range->isChecked()) {
            m_rangeField->setFocus();
        }
        return;
    }
    QDialog::accept();
}

} // namespace vellora
