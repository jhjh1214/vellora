// The command palette (Ctrl+Shift+P): a popup with a search box over the registry's commands. Type
// to filter (every word must appear in the title), Up/Down to choose, Enter to run, Esc to close.
#pragma once

#include "commands/CommandRegistry.h"

#include <QDialog>

class QLineEdit;
class QListWidget;

namespace vellora {

class CommandPalette : public QDialog {
    Q_OBJECT

public:
    explicit CommandPalette(const CommandRegistry* registry, QWidget* parent = nullptr);

    // Shows the palette centred near the top of the parent window with an empty search.
    void open();

    // Ids of the commands currently listed, in order, and the one chosen. For tests.
    QStringList visibleCommandIds() const;
    QString currentCommandId() const;

    // Runs the chosen command and closes the palette. False if nothing is chosen.
    bool runCurrent();

protected:
    bool eventFilter(QObject* watched, QEvent* event) override;

private:
    void refresh(const QString& filter);

    const CommandRegistry* m_registry;
    QLineEdit* m_search;
    QListWidget* m_list;
};

} // namespace vellora
