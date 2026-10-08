#include "commands/CommandRegistry.h"

#include <QAction>
#include <QWidget>
#include <utility>

namespace vellora {

bool CommandRegistry::add(Command command) {
    if (command.id.isEmpty() || command.title.isEmpty() || !command.handler ||
        find(command.id) != nullptr) {
        return false;
    }
    m_commands.append(std::move(command));
    return true;
}

const Command* CommandRegistry::find(const QString& id) const {
    for (const Command& command : m_commands) {
        if (command.id == id) {
            return &command;
        }
    }
    return nullptr;
}

bool CommandRegistry::run(const QString& id) const {
    const Command* command = find(id);
    if (command == nullptr) {
        return false;
    }
    // Copied: the handler may add commands, which can move the list.
    const std::function<void()> handler = command->handler;
    handler();
    return true;
}

QAction* CommandRegistry::createAction(const QString& id, QWidget* owner) const {
    const Command* command = find(id);
    if (command == nullptr) {
        return nullptr;
    }
    auto* action = new QAction(command->title, owner);
    action->setShortcuts(command->shortcuts);
    // The registry outlives its actions' use: it is a member of the window that owns them.
    connect(action, &QAction::triggered, this, [this, id] { run(id); });
    owner->addAction(action);
    return action;
}

} // namespace vellora
