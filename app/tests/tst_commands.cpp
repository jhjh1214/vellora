// The command registry, the actions and menus derived from it, and the command palette.
#include "MainWindow.h"
#include "commands/CommandPalette.h"
#include "commands/CommandRegistry.h"

#include <QAction>
#include <QApplication>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QMenuBar>
#include <QSet>
#include <QSignalSpy>
#include <QTest>
#include <functional>

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
        // The Open Recent submenu holds recent files, which are not commands, and its two fixed
        // entries, which are; the files come from the settings and there are none here. A
        // submenu that only groups entries (Page Layout, Zoom) is not a command either.
        int entries = 0;
        const std::function<void(const QMenu*)> check = [&](const QMenu* menu) {
            for (const QAction* entry : menu->actions()) {
                if (entry->isSeparator()) {
                    continue;
                }
                bool found = false;
                for (const Command& command : registry.commands()) {
                    found = found || (command.title == entry->text() &&
                                      command.shortcuts == entry->shortcuts());
                }
                if (entry->menu() == nullptr || found) {
                    QVERIFY2(found, qPrintable(entry->text()));
                    ++entries;
                }
                if (entry->menu() != nullptr) {
                    if (entry->menu() == window.recentMenu()) {
                        window.refreshRecentMenu();
                    }
                    check(entry->menu());
                }
            }
        };
        for (const QAction* menuAction : window.menuBar()->actions()) {
            check(menuAction->menu());
        }
        QCOMPARE(entries, registry.commands().size());
    }

    // Every command of the application, as the window registers them.
    void everyCommandHasATitleAndAWayToReachItFromTheKeyboard() {
        vellora::MainWindow window;
        window.show();
        const QList<Command>& commands = window.commands().commands();
        QVERIFY(commands.size() > 40);
        QSet<QString> ids;
        QSet<QString> titles;
        for (const Command& command : commands) {
            // A title is what a screen reader reads and the palette lists: plain, not empty, and
            // not the same as another command's.
            QVERIFY2(!command.id.isEmpty(), "a command without an id");
            QVERIFY2(!command.title.trimmed().isEmpty(), qPrintable(command.id + ": no title"));
            QVERIFY2(command.title == command.title.trimmed(), qPrintable(command.id));
            QVERIFY2(!command.title.contains(QLatin1Char('&')) &&
                         !command.title.contains(QLatin1Char('\n')),
                     qPrintable(command.id + ": title is not plain text"));
            QVERIFY2(!ids.contains(command.id), qPrintable(command.id + ": id used twice"));
            QVERIFY2(!titles.contains(command.title),
                     qPrintable(command.title + ": title used twice"));
            ids.insert(command.id);
            titles.insert(command.title);
            QVERIFY2(command.handler != nullptr, qPrintable(command.id));
        }

        // The keyboard reaches all of them through the palette, which has a shortcut of its own,
        // lists every command and shows each one's shortcut next to its title.
        const Command* palette = window.commands().find(QStringLiteral("palette.show"));
        QVERIFY(palette != nullptr && !palette->shortcuts.isEmpty());
        window.palette().open();
        QCOMPARE(window.palette().visibleCommandIds().size(), commands.size());
        auto* list = window.palette().findChild<QListWidget*>();
        QVERIFY(list != nullptr);
        for (int row = 0; row < list->count(); ++row) {
            const Command* command =
                window.commands().find(list->item(row)->data(Qt::UserRole).toString());
            QVERIFY(command != nullptr);
            const QString text = list->item(row)->text();
            QVERIFY2(text.startsWith(command->title), qPrintable(text));
            if (!command->shortcuts.isEmpty()) {
                QVERIFY2(
                    text.endsWith(command->shortcuts.first().toString(QKeySequence::NativeText)),
                    qPrintable(text));
            }
        }

        // No two commands claim the same key.
        QSet<QString> keys;
        for (const Command& command : commands) {
            for (const QKeySequence& key : command.shortcuts) {
                const QString text = key.toString(QKeySequence::PortableText);
                QVERIFY2(!keys.contains(text), qPrintable(text + " is used twice"));
                keys.insert(text);
            }
        }
    }

    void thePaletteStillFitsATitleAndItsShortcutWithALargerSystemFont() {
        QFont large = QApplication::font();
        const QFont original = large;
        large.setPointSizeF(original.pointSizeF() * 2.0);
        QApplication::setFont(large);
        {
            vellora::MainWindow window;
            window.show();
            window.palette().open();
            auto* list = window.palette().findChild<QListWidget*>();
            QVERIFY(list != nullptr);
            int widest = 0;
            for (int row = 0; row < list->count(); ++row) {
                widest = std::max(widest,
                                  list->fontMetrics().horizontalAdvance(list->item(row)->text()));
            }
            QVERIFY2(widest > 0, "no rows");
            QVERIFY2(widest <= list->viewport()->width(),
                     qPrintable(QStringLiteral("the longest row is %1 pixels, the list %2")
                                    .arg(widest)
                                    .arg(list->viewport()->width())));
        }
        QApplication::setFont(original);
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
        // The zoom commands, presets included, in registration order.
        QCOMPARE(palette.visibleCommandIds().mid(0, 2),
                 (QStringList{QStringLiteral("view.zoomIn"), QStringLiteral("view.zoomOut")}));
        QVERIFY(palette.visibleCommandIds().contains(QStringLiteral("view.zoom.6400")));
        QVERIFY(palette.visibleCommandIds().contains(QStringLiteral("view.zoomToSelection")));
        // Words may come in any order and any case.
        search->setText(QStringLiteral("OUT zoom"));
        QCOMPARE(palette.visibleCommandIds(), QStringList{QStringLiteral("view.zoomOut")});
        search->setText(QStringLiteral("nothing like this"));
        QVERIFY(palette.visibleCommandIds().isEmpty());
        QVERIFY(!palette.runCurrent());

        // Up and Down choose, Return runs the choice and closes the palette.
        search->setText(QStringLiteral("fit"));
        QCOMPARE(palette.visibleCommandIds(),
                 (QStringList{QStringLiteral("view.fitWidth"), QStringLiteral("view.fitPage")}));
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.fitWidth"));
        QTest::keyClick(search, Qt::Key_Down);
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.fitPage"));
        QTest::keyClick(search, Qt::Key_Down);
        QCOMPARE(palette.currentCommandId(), QStringLiteral("view.fitWidth")); // wraps
        QTest::keyClick(search, Qt::Key_Down);
        QTest::keyClick(search, Qt::Key_Return);
        QVERIFY(!palette.isVisible());
        QCOMPARE(window.canvas().controller()->zoomMode(),
                 vellora::CanvasController::ZoomMode::FitPage);
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
