// The third-party licenses viewer: a searchable list of every bundled component (the Rust crates,
// PDFium and the libraries inside it, Qt) next to the full text of `THIRD_PARTY_LICENSES`, which is
// embedded in the executable. Choosing a component scrolls the text to its notice. Plain text only.
#pragma once

#include <QDialog>
#include <QStringList>

class QLineEdit;
class QListWidget;
class QPlainTextEdit;

namespace vellora {

class LicensesDialog : public QDialog {
    Q_OBJECT

public:
    // The embedded file.
    static constexpr auto kResource = ":/licenses/THIRD_PARTY_LICENSES";

    explicit LicensesDialog(QWidget* parent = nullptr, const QString& resource = kResource);

    // Every component, sorted: "name version" for a Rust crate, "name (in PDFium)" for a library
    // PDFium bundles, and "PDFium" and "Qt" themselves.
    QStringList components() const { return m_components; }
    // Shows only the components that contain `text` (case-insensitive); empty shows all.
    void setFilter(const QString& text);
    QStringList visibleComponents() const;
    // Scrolls the text to the notice of `component` (one of `components()`); false if unknown.
    bool select(const QString& component);

    QPlainTextEdit* viewer() const { return m_viewer; }
    QListWidget* list() const { return m_list; }
    QLineEdit* filter() const { return m_filter; }

private:
    QStringList m_components;
    QListWidget* m_list = nullptr;
    QLineEdit* m_filter = nullptr;
    QPlainTextEdit* m_viewer = nullptr;
};

} // namespace vellora