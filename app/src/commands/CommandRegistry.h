// Every user action is a registered command: an id, a title, shortcuts and a handler. Menus, window
// shortcuts and the command palette are all derived from this one list, so an action cannot exist
// in one place and be missing in another.
#pragma once

#include <QKeySequence>
#include <QList>
#include <QObject>
#include <QString>
#include <functional>

class QAction;
class QWidget;

namespace vellora {

struct Command {
    // Stable, dotted, lower camel case ("view.zoomIn"). Unique.
    QString id;
    // What menus and the palette show: plain text, no mnemonics.
    QString title;
    QList<QKeySequence> shortcuts;
    std::function<void()> handler;
};

class CommandRegistry : public QObject {
    Q_OBJECT

public:
    explicit CommandRegistry(QObject* parent = nullptr) : QObject(parent) {}

    // Adds a command. Refuses (false) an empty id, title or handler and an id already taken.
    bool add(Command command);

    // In registration order, which is the order the palette lists them in.
    const QList<Command>& commands() const { return m_commands; }
    const Command* find(const QString& id) const;
    // Runs the command; false if there is none with that id.
    bool run(const QString& id) const;

    // A QAction for the command, owned by `owner` and added to it so that the shortcuts work in
    // the whole window. Null if the id is unknown.
    QAction* createAction(const QString& id, QWidget* owner) const;

private:
    QList<Command> m_commands;
};

} // namespace vellora
