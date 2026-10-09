#include "LicensesDialog.h"

#include <QFile>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QListWidget>
#include <QPlainTextEdit>
#include <QRegularExpression>
#include <QScrollBar>
#include <QTextCursor>
#include <QVBoxLayout>
#include <algorithm>

namespace vellora {

namespace {

constexpr auto kPdfiumMarker = "PDFium bundled license texts";
constexpr auto kBundledMarker = "Bundled in the PDFium build:";

QString readResource(const QString& path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        return {};
    }
    return QString::fromUtf8(file.readAll());
}

QString pdfiumLibrary(const QString& name) {
    return QStringLiteral("%1 (in PDFium)").arg(name);
}

// The components named in the file: the "Used by" lists of the crates, then the binary components
// described in the notices (PDFium, the libraries inside it, Qt).
QStringList parseComponents(const QString& text) {
    QStringList components;
    // Only the lists that follow a "Used by:" line: license texts have lists of their own.
    static const QRegularExpression crate(QStringLiteral("^  - (\\S+) (\\S+)(?: \\(.*\\))?$"));
    bool inList = false;
    for (const QStringView line : QStringView(text).split(QLatin1Char('\n'))) {
        if (line == QLatin1String("Used by:")) {
            inList = true;
            continue;
        }
        const QRegularExpressionMatch match =
            inList ? crate.matchView(line) : QRegularExpressionMatch();
        if (match.hasMatch()) {
            components.append(match.captured(1) + QLatin1Char(' ') + match.captured(2));
        } else {
            inList = false;
        }
    }
    // "Bundled in the PDFium build: a, b, c." may wrap over lines; it ends at the full stop.
    const qsizetype bundled = text.indexOf(QLatin1String(kBundledMarker));
    if (bundled >= 0) {
        const qsizetype start = bundled + QLatin1String(kBundledMarker).size();
        const qsizetype end = text.indexOf(QLatin1Char('.'), start);
        const QStringList names =
            text.mid(start, end - start).simplified().split(QLatin1Char(','), Qt::SkipEmptyParts);
        for (const QString& name : names) {
            components.append(pdfiumLibrary(name.trimmed()));
        }
    }
    if (text.contains(QLatin1String("\nPDFium\n"))) {
        components.append(QStringLiteral("PDFium"));
    }
    if (text.contains(QLatin1String("\nQt\n"))) {
        components.append(QStringLiteral("Qt"));
    }
    components.removeDuplicates();
    std::sort(components.begin(), components.end(), [](const QString& a, const QString& b) {
        return a.compare(b, Qt::CaseInsensitive) < 0;
    });
    return components;
}

} // namespace

LicensesDialog::LicensesDialog(QWidget* parent, const QString& resource) : QDialog(parent) {
    setObjectName(QStringLiteral("licensesDialog"));
    setWindowTitle(tr("Third-party licenses"));

    const QString text = readResource(resource);
    m_components = parseComponents(text);

    m_filter = new QLineEdit(this);
    m_filter->setPlaceholderText(tr("Filter components"));
    m_filter->setClearButtonEnabled(true);
    m_list = new QListWidget(this);
    m_list->addItems(m_components);
    m_viewer = new QPlainTextEdit(this);
    m_viewer->setReadOnly(true);
    m_viewer->setLineWrapMode(QPlainTextEdit::NoWrap);
    m_viewer->setPlainText(text.isEmpty() ? tr("The license file is missing from this build.")
                                          : text);

    auto* left = new QVBoxLayout;
    left->addWidget(m_filter);
    left->addWidget(m_list, 1);
    auto* layout = new QHBoxLayout(this);
    layout->addLayout(left, 1);
    layout->addWidget(m_viewer, 3);
    resize(900, 560);

    connect(m_filter, &QLineEdit::textChanged, this, &LicensesDialog::setFilter);
    connect(m_list, &QListWidget::currentTextChanged, this,
            [this](const QString& component) { select(component); });
}

void LicensesDialog::setFilter(const QString& text) {
    for (int i = 0; i < m_list->count(); ++i) {
        m_list->item(i)->setHidden(!m_list->item(i)->text().contains(text, Qt::CaseInsensitive));
    }
}

QStringList LicensesDialog::visibleComponents() const {
    QStringList visible;
    for (int i = 0; i < m_list->count(); ++i) {
        if (!m_list->item(i)->isHidden()) {
            visible.append(m_list->item(i)->text());
        }
    }
    return visible;
}

bool LicensesDialog::select(const QString& component) {
    if (!m_components.contains(component)) {
        return false;
    }
    const QString text = m_viewer->toPlainText();
    qsizetype at = -1;
    if (component == QLatin1String("PDFium")) {
        at = text.indexOf(QLatin1String("\nPDFium\n"));
    } else if (component == QLatin1String("Qt")) {
        at = text.indexOf(QLatin1String("\nQt\n"));
    } else if (component.endsWith(QLatin1String(" (in PDFium)"))) {
        // The license texts are headed "--- name.txt ---", with "_" where the library has "-".
        const QString name = component.left(component.size() - 12);
        const qsizetype texts = text.indexOf(QLatin1String(kPdfiumMarker));
        for (const QString& variant :
             {name, QString(name).replace(QLatin1Char('-'), QLatin1Char('_'))}) {
            at = text.indexOf(QStringLiteral("--- %1").arg(variant), std::max<qsizetype>(texts, 0));
            if (at >= 0) {
                break;
            }
        }
    } else {
        at = text.indexOf(QStringLiteral("\n  - %1").arg(component));
    }
    if (at < 0) {
        return false;
    }
    QTextCursor cursor(m_viewer->document());
    cursor.setPosition(static_cast<int>(at) + 1);
    m_viewer->setTextCursor(cursor);
    // Put the notice at the top of the view rather than at its bottom edge.
    m_viewer->verticalScrollBar()->setValue(m_viewer->verticalScrollBar()->maximum());
    m_viewer->setTextCursor(cursor);
    m_viewer->ensureCursorVisible();
    return true;
}

} // namespace vellora