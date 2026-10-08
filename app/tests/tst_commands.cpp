// The command registry, the actions and menus derived from it, and the command palette.
#include "MainWindow.h"
#include "commands/CommandPalette.h"
#include "commands/CommandRegistry.h"

#include <QAction>
#include <QLineEdit>
#include <QListWidget>
#include <QMenuBar>
#include <QSignalSpy>
#include <QTest>

using vellora::Command;
using vellora::CommandRegistry;

class TstCommands : public QObject {
    Q_OBJECT

private slots:
    void aCommandNeedsAnIdATitleAndAHandler() {
        CommandRegistry registry;
        const auto noop = [] {};
        QVERIFY(!registry.add({QString(), QStringLiteral("T"), {}, noop}));
        QVERIFY(!registry.add({QStringLiteral("a.b"), QString(), {}, noop}));
        QVERIFY(!registry.add({QStringLiteral("a.b"), QStringLiteral("T"), {}, nullptr}));
        QVERIFY(registry.commands().isEmpty());
        QVERIFY(registry.add({QStringLiteral("a.b"), QStringLiteral("T"), {}, noop}));
        // Ids are unique.
        QVERIFY(!registry.add({QStringLiteral("a.b"), QStringLiteral("Other"), {}, noop}));
        QCOMPARE(registry.commands().size(), 1);
    }

    void runningACommandCallsItsHandler() {
        CommandRegistry registry;
        int calls = 0;
        QVERIFY(registry.add({QStringLiteral("x"), QStringLiteral("X"), {}, [&] { ++calls; }}));
        QVERIFY(registry.run(QStringLiteral("x")));
        QCOMPARE(calls, 1);
        QVERIFY(!registry.run(QStringLiteral("nope")));
        QCOMPARE(calls, 1);
        QCOMPARE(registry.find(QStringLiteral("x"))->title, QStringLiteral("X"));
        QVERIFY(registry.find(QStringLiteral("nope")) == nullptr);
    }

    void anActionCarriesTheTitleAndShortcutsAndRunsTheCommand() {
        CommandRegistry registry;
        int calls = 0;
        const QList<QKeySequence> keys{QKeySequence(Qt::CTRL | Qt::Key_K),
                                       QKeySequence(Qt::CTRL | Qt::Key_L)};
        QVERIFY(
            registry.add({QStringLiteral("x"), QStringLiteral("Do X"), keys, [&] { ++calls; }}));
        QWidget owner;
        QAction* action = registry.createAction(QStringLiteral("x"), &owner);
        QVERIFY(action != nullptr);
        QCOMPARE(action->text(), QStringLiteral("Do X"));
        QCOMPARE(action->shortcuts(), keys);
        QVERIFY(owner.actions().contains(action)); // shortcuts work in the whole window
        action->trigger();
        QCOMPARE(calls, 1);
        QVERIFY(registry.createAction(QStringLiteral("nope"), &owner) == nullptr);
    }

    void theWindowRegistersItsActionsAndItsMenusAreDerivedFromThem() {
        vellora::MainWindow window;
        const CommandRegistry& registry = window.commands();
        for (const char* id : {"file.open", "view.zoomIn", "view.zoomOut", "palette.show"}) {
            QVERIFY2(registry.find(QString::fromLatin1(id)) != nullptr, id);
        }

        // Every menu entry is the action of a registered command: same title, same shortcuts.
        int entries = 0;
        for (const QAction* menuAction : window.menuBar()->actions()) {
            for (const QAction* entry : menuAction->menu()->actions()) {
                if (entry->isSeparator()) {
                    continue;
                }
                ++entries;
                bool found = false;
                for (const Command& command : registry.commands()) {
                    found = found || (command.title == entry->text() &&
                                      command.shortcuts == entry->shortcuts());
                }
                QVERIFY2(found, qPrintable(entry->text()));
            }
        }
        QCOMPARE(entries, registry.commands().size());
    }

    void commandsDriveTheCanvas() {
        vellora::MainWindow window;
        const double before = window.canvas().controller()->zoom();
        QVERIFY(window.commands().run(QStringLiteral("view.zoomIn")));
        QVERIFY(window.canvas().controller()->zoom() > before);
        QVERIFY(window.commands().run(QStringLiteral("view.zoomOut")));
        QVERIFY(qAbs(window.canvas().controller()->zoom() - before) < 1e-9);
    }

    void thePaletteListsFiltersAndRunsCommands() {
        vellora::MainWindow window;
        window.show();
        vellora::CommandPalette& palette = window.palette();
        palette.open();
        QVERIFY(palette.isVisible());
        // Everything, in registration order.
        QStringList all;
        for (const Command& command : window.commands().commands()) {
            all.append(command.id);
        }
        QCOMPARE(palette.visibleCommandIds(), all);

        auto* search = palette.findChild<QLineEdit*>();
        QVERIFY(search != nullptr);
        QTest::keyClicks(search, QStringLiteral("zoom"));
        QCOMPARE(palette.visibleCommandIds(),
                 (QStringList{QStringLiteral("view.zoomIn"), QStringLiteral("view.zoomOut")}));
        // Words may come in any order and any case.
        search->setText(QStringLiteral("OUT zoom"));
        QCOMPARE(palette.visibleCommandIds(), QStringList{QStringLiteral("view.zoomOut")});
        search->setText(QStringLiteral("nothing like this"));
        QVERIFY(palette.visibleCommandIds().isEmpty());
        QVERIFY(!palette.runCurrent());

        // Up and Down choose, Return runs the choice and closes the palette.
        search->setText(QStringLiteral("zoom"));
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.zoomIn"));
        QTest::keyClick(search, Qt::Key_Down);
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.zoomOut"));
        QTest::keyClick(search, Qt::Key_Down);
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.zoomIn")); // wraps
        const double before = window.canvas().controller()->zoom();
        QTest::keyClick(search, Qt::Key_Return);
        QVERIFY(!palette.isVisible());
        QVERIFY(window.canvas().controller()->zoom() > before);
    }

    void thePaletteOpensFromItsOwnCommandAndClosesOnEscape() {
        vellora::MainWindow window;
        window.show();
        QVERIFY(window.commands().run(QStringLiteral("palette.show")));
        QVERIFY(window.palette().isVisible());
        QTest::keyClick(window.palette().findChild<QLineEdit*>(), Qt::Key_Escape);
        QVERIFY(!window.palette().isVisible());
    }
};

QTEST_MAIN(TstCommands)
#include "tst_commands.moc"
