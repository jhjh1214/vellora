// Tabs: one engine per document, opening files (command line, drop, recent list), the commands of
// the tab bar, and putting a document back where it was left.
#include "KillProcess.h"
#include "MainWindow.h"
#include "RepairBar.h"
#include "settings/AppSettings.h"

#include <QAction>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QFile>
#include <QMenu>
#include <QMimeData>
#include <QSettings>
#include <QSignalSpy>
#include <QTabWidget>
#include <QTemporaryDir>
#include <QTest>
#include <QUrl>

namespace {

constexpr int kWaitMs = 60'000;

// A copy of the golden document under `name`: a different file, so a different tab.
QString copyGolden(const QTemporaryDir& dir, const QString& name) {
    const QString path = dir.filePath(name);
    return QFile::copy(QStringLiteral(VELLORA_GOLDEN_PDF), path) ? path : QString();
}

QTabWidget* tabWidget(vellora::MainWindow& window) {
    return window.findChild<QTabWidget*>();
}

} // namespace

class TstTabs : public QObject {
    Q_OBJECT

private slots:
    void aNewWindowHasOneEmptyTab() {
        vellora::MainWindow window;
        QCOMPARE(window.tabCount(), 1);
        QVERIFY(window.currentTab().isEmpty());
        QCOMPARE(tabWidget(window)->tabText(0), QStringLiteral("New Tab"));
        // Closing the only tab leaves an empty one: the window never has none.
        window.closeCurrentTab();
        QCOMPARE(window.tabCount(), 1);
        QVERIFY(window.currentTab().isEmpty());
        QVERIFY(window.closedTabs().isEmpty());
    }

    void filesFillTheEmptyTabThenOpenNewOnesAndAnOpenFileJustComesToTheFront() {
        QTemporaryDir dir;
        const QString a = copyGolden(dir, QStringLiteral("a.pdf"));
        const QString b = copyGolden(dir, QStringLiteral("b&c.pdf"));
        vellora::MainWindow window;
        window.show();

        QVERIFY(window.openDocument(a));
        QCOMPARE(window.tabCount(), 1); // the empty tab was used
        QVERIFY(window.openDocument(b));
        QCOMPARE(window.tabCount(), 2);
        QCOMPARE(window.currentTabIndex(), 1);
        QCOMPARE(tabWidget(window)->tabText(0), QStringLiteral("a.pdf"));
        // The file name is text: an ampersand must not become a mnemonic.
        QCOMPARE(tabWidget(window)->tabText(1), QStringLiteral("b&&c.pdf"));
        QCOMPARE(tabWidget(window)->tabToolTip(1), b);

        // The same file again switches to its tab, whatever way the path is written.
        QVERIFY(window.openDocument(dir.filePath(QStringLiteral("x/../a.pdf"))));
        QCOMPARE(window.tabCount(), 2);
        QCOMPARE(window.currentTabIndex(), 0);
        QTRY_COMPARE_WITH_TIMEOUT(window.documentStatus(), QStringLiteral("3 page(s)"), kWaitMs);
        QCOMPARE(window.windowTitle(), QStringLiteral("a.pdf — Vellora"));
        window.setCurrentTabIndex(1);
        QTRY_COMPARE_WITH_TIMEOUT(window.documentStatus(), QStringLiteral("3 page(s)"), kWaitMs);
        QCOMPARE(window.windowTitle(), QStringLiteral("b&c.pdf — Vellora"));
    }

    void eachTabKeepsItsOwnStatus() {
        QTemporaryDir dir;
        vellora::MainWindow window;
        window.resize(500, 250);
        window.show();
        QVERIFY(window.openDocument(copyGolden(dir, QStringLiteral("a.pdf"))));
        QVERIFY(window.openDocument(copyGolden(dir, QStringLiteral("b.pdf"))));
        QTRY_COMPARE_WITH_TIMEOUT(window.tab(0).documentStatus(), QStringLiteral("3 page(s)"),
                                  kWaitMs);
        QTRY_COMPARE_WITH_TIMEOUT(window.tab(1).documentStatus(), QStringLiteral("3 page(s)"),
                                  kWaitMs);
        window.tab(0).canvas().zoomIn();
        QCOMPARE(window.zoomStatus(), QStringLiteral("100%")); // the current tab is the second
        window.setCurrentTabIndex(0);
        QCOMPARE(window.zoomStatus(), QStringLiteral("125%"));
        QCOMPARE(&window.session(), &window.tab(0).session());
    }

    void theTabCommandsMoveCloseAndReopen() {
        QTemporaryDir dir;
        const QString a = copyGolden(dir, QStringLiteral("a.pdf"));
        const QString b = copyGolden(dir, QStringLiteral("b.pdf"));
        const QString c = copyGolden(dir, QStringLiteral("c.pdf"));
        vellora::MainWindow window;
        window.show();
        QCOMPARE(window.openDocuments({a, b, c}), QStringList());
        QCOMPARE(window.tabCount(), 3);
        QCOMPARE(window.currentTabIndex(), 2);

        // Registered commands with the shortcuts the task names.
        const auto shortcutsOf = [&](const char* id) {
            return window.commands().find(QString::fromLatin1(id))->shortcuts;
        };
        QCOMPARE(shortcutsOf("tabs.next"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::Key_Tab)}));
        QCOMPARE(shortcutsOf("tabs.previous"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_Tab)}));
        QCOMPARE(shortcutsOf("file.close"), (QList<QKeySequence>{QKeySequence::Close}));
        QCOMPARE(shortcutsOf("file.reopenClosed"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_T)}));

        QVERIFY(window.commands().run(QStringLiteral("tabs.next"))); // wraps to the first
        QCOMPARE(window.currentTabIndex(), 0);
        QVERIFY(window.commands().run(QStringLiteral("tabs.previous"))); // and back to the last
        QCOMPARE(window.currentTabIndex(), 2);

        // Close the middle one, then the last: they come back newest first.
        window.setCurrentTabIndex(1);
        QVERIFY(window.commands().run(QStringLiteral("file.close")));
        QCOMPARE(window.tabCount(), 2);
        QCOMPARE(window.closedTabs(), (QStringList{b}));
        window.closeTab(1);
        QCOMPARE(window.closedTabs(), (QStringList{b, c}));
        QVERIFY(window.commands().run(QStringLiteral("file.reopenClosed")));
        QCOMPARE(window.tabCount(), 2);
        QCOMPARE(window.currentTab().path(), c);
        QVERIFY(window.reopenClosedTab());
        QCOMPARE(window.currentTab().path(), b);
        QVERIFY(!window.reopenClosedTab()); // nothing left
        QCOMPARE(window.tabCount(), 3);
    }

    void aClosedFileThatIsGoneIsSkippedAndTheListIsBounded() {
        QTemporaryDir dir;
        const QString a = copyGolden(dir, QStringLiteral("a.pdf"));
        const QString b = copyGolden(dir, QStringLiteral("b.pdf"));
        vellora::MainWindow window;
        window.openDocuments({a, b});
        window.closeTab(1);
        window.closeTab(0);
        QCOMPARE(window.closedTabs(), (QStringList{b, a}));
        QVERIFY(QFile::remove(a));
        // a was closed last but is gone: b is reopened instead.
        QVERIFY(window.reopenClosedTab());
        QCOMPARE(window.currentTab().path(), b);

        // Only the last `kMaxClosedTabs` are remembered.
        for (int i = 0; i < vellora::MainWindow::kMaxClosedTabs + 5; ++i) {
            QVERIFY(window.openDocument(copyGolden(dir, QStringLiteral("n%1.pdf").arg(i))));
            window.closeCurrentTab();
        }
        QCOMPARE(window.closedTabs().size(), vellora::MainWindow::kMaxClosedTabs);
        QVERIFY(window.closedTabs().last().endsWith(
            QStringLiteral("n%1.pdf").arg(vellora::MainWindow::kMaxClosedTabs + 4)));
    }

    void closingTabsEndsTheirEnginesWithoutLeaks() {
        QTemporaryDir dir;
        vellora::MainWindow window;
        window.show();
        QCOMPARE(vellora::EngineSession::liveCount(), 1);
        int closed = 0;
        for (int round = 0; round < 5; ++round) {
            QList<quint32> engines;
            for (int i = 0; i < 10; ++i) {
                QVERIFY(window.openDocument(
                    copyGolden(dir, QStringLiteral("r%1-%2.pdf").arg(round).arg(i))));
                engines.append(window.session().engineProcessId());
                QVERIFY(engines.last() != 0);
            }
            // Ten documents, ten engines (the first tab of round 0 was the empty one).
            QCOMPARE(vellora::EngineSession::liveCount(), window.tabCount());
            for (const quint32 pid : std::as_const(engines)) {
                QVERIFY2(processExists(pid), qPrintable(QString::number(pid)));
            }
            while (window.tabCount() > 1 || !window.currentTab().isEmpty()) {
                window.closeCurrentTab();
                ++closed;
            }
            // Closing a tab waits for its engine to leave: none is left running.
            for (const quint32 pid : std::as_const(engines)) {
                QVERIFY2(!processExists(pid), qPrintable(QString::number(pid)));
            }
            QCOMPARE(vellora::EngineSession::liveCount(), 1);
        }
        QCOMPARE(closed, 50);
    }

    void closingTheWindowEndsEveryEngine() {
        QTemporaryDir dir;
        QList<quint32> engines;
        {
            vellora::MainWindow window;
            QVERIFY(window.openDocument(copyGolden(dir, QStringLiteral("a.pdf"))));
            QVERIFY(window.openDocument(copyGolden(dir, QStringLiteral("b.pdf"))));
            for (int i = 0; i < 2; ++i) {
                engines.append(window.tab(i).session().engineProcessId());
            }
        }
        for (const quint32 pid : std::as_const(engines)) {
            QVERIFY(!processExists(pid));
        }
        QCOMPARE(vellora::EngineSession::liveCount(), 0);
    }

    void aFileThatCannotBeOpenedIsReportedAndDoesNotBlockTheNext() {
        QTemporaryDir dir;
        const QString good = copyGolden(dir, QStringLiteral("good.pdf"));
        vellora::MainWindow window;
        const QString missing = dir.filePath(QStringLiteral("missing.pdf"));
        QCOMPARE(window.openDocuments({missing, good}), (QStringList{missing}));
        QVERIFY(window.tab(0).documentStatus().startsWith(QStringLiteral("Cannot open")));
        QCOMPARE(window.tabCount(), 2);
        QCOMPARE(window.currentTab().path(), good);
    }

    void droppedFilesOpenInTabs() {
        QTemporaryDir dir;
        const QString a = copyGolden(dir, QStringLiteral("a.pdf"));
        const QString b = copyGolden(dir, QStringLiteral("b.pdf"));
        vellora::MainWindow window;
        window.show();

        QMimeData text;
        text.setText(QStringLiteral("not a file"));
        QDragEnterEvent refused(QPoint(10, 10), Qt::CopyAction, &text, Qt::LeftButton,
                                Qt::NoModifier);
        QCoreApplication::sendEvent(&window, &refused);
        QVERIFY(!refused.isAccepted());

        QMimeData files;
        files.setUrls({QUrl::fromLocalFile(a), QUrl(QStringLiteral("https://example.org/x.pdf")),
                       QUrl::fromLocalFile(b)});
        QDragEnterEvent enter(QPoint(10, 10), Qt::CopyAction, &files, Qt::LeftButton,
                              Qt::NoModifier);
        QCoreApplication::sendEvent(&window, &enter);
        QVERIFY(enter.isAccepted());
        QDropEvent drop(QPointF(10, 10), Qt::CopyAction, &files, Qt::LeftButton, Qt::NoModifier);
        QCoreApplication::sendEvent(&window, &drop);
        QCOMPARE(window.tabCount(), 2); // the web address is not a file
        QCOMPARE(window.tab(0).path(), a);
        QCOMPARE(window.tab(1).path(), b);
    }

    void theRecentMenuListsFilesGreysOutMissingOnesAndCanBeCleared() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        vellora::AppSettings settings(&backing);
        const QString a = copyGolden(dir, QStringLiteral("a.pdf"));
        const QString b = copyGolden(dir, QStringLiteral("b.pdf"));
        const QString c = copyGolden(dir, QStringLiteral("c.pdf"));
        vellora::MainWindow window(&settings);
        window.openDocuments({a, b, c});
        QCOMPARE(settings.recentFiles(), (QStringList{c, b, a}));
        QCOMPARE(settings.lastDirectory(), dir.path());

        const auto entries = [&] {
            window.refreshRecentMenu();
            QList<QAction*> files;
            for (QAction* action : window.recentMenu()->actions()) {
                if (!action->isSeparator() && action->toolTip().endsWith(QStringLiteral(".pdf"))) {
                    files.append(action);
                }
            }
            return files;
        };
        QCOMPARE(entries().size(), 3);
        QCOMPARE(entries().at(0)->text(), QStringLiteral("c.pdf"));
        QCOMPARE(entries().at(0)->toolTip(), c);
        for (const QAction* action : entries()) {
            QVERIFY(action->isEnabled());
        }

        // Choosing an entry opens it (here: it is already open, so its tab comes to the front).
        window.setCurrentTabIndex(0);
        entries().at(0)->trigger();
        QCOMPARE(window.currentTab().path(), c);

        // A file that is gone is greyed out, not hidden...
        window.closeTab(1);
        QVERIFY(QFile::remove(b));
        const QList<QAction*> afterRemoval = entries();
        QCOMPARE(afterRemoval.size(), 3);
        QVERIFY(afterRemoval.at(0)->isEnabled());
        QVERIFY(!afterRemoval.at(1)->isEnabled());
        QVERIFY(afterRemoval.at(2)->isEnabled());
        // ...and "Remove Unavailable Files" takes it off the list.
        QVERIFY(window.commands().run(QStringLiteral("file.removeUnavailableRecent")));
        QCOMPARE(settings.recentFiles(), (QStringList{c, a}));
        QCOMPARE(entries().size(), 2);

        QVERIFY(window.commands().run(QStringLiteral("file.clearRecent")));
        QVERIFY(settings.recentFiles().isEmpty());
        QCOMPARE(entries().size(), 0);
        // The fixed entries stay.
        QVERIFY(window.recentMenu()->actions().size() >= 2);
    }

    void aDocumentComesBackWhereItWasLeft() {
        QTemporaryDir dir;
        const QString ini = dir.filePath(QStringLiteral("s.ini"));
        const QString path = copyGolden(dir, QStringLiteral("a.pdf"));
        struct Left {
            quint32 page = 0;
            double offsetPoints = 0.0;
            double zoom = 1.0;
        } left;
        {
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.resize(500, 250);
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(path));
            QVERIFY(opened.wait(kWaitMs));
            window.canvas().zoomIn();
            window.canvas().zoomIn();
            // Down into the second page.
            const double y = window.canvas().controller()->layout().pageTop(1, 1.5625) + 40.0;
            window.canvas().controller()->setScrollPosition(QPointF(0.0, y));
            left = {window.canvas().controller()->topAnchor().page,
                    window.canvas().controller()->topAnchor().offsetPoints,
                    window.canvas().controller()->zoom()};
            QVERIFY(left.page >= 1);
            window.closeTab(0);
        }
        // Another run of the application, another settings object over the same file.
        {
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            QCOMPARE(settings.viewStateCount(), 1);
            vellora::MainWindow window(&settings);
            window.resize(500, 250);
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(path));
            QVERIFY(opened.wait(kWaitMs));
            QCOMPARE(window.canvas().controller()->zoom(), left.zoom);
            QCOMPARE(window.canvas().controller()->topAnchor().page, left.page);
            QVERIFY(qAbs(window.canvas().controller()->topAnchor().offsetPoints -
                         left.offsetPoints) < 1.0);
            QCOMPARE(window.zoomStatus(), QStringLiteral("156%"));
        }
        // A file that changed on disk is another file: it opens from the top.
        {
            QFile file(path);
            QVERIFY(file.open(QIODevice::Append));
            file.write("\n% appended\n");
            file.close();
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.resize(500, 250);
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(path));
            QVERIFY(opened.wait(kWaitMs));
            QCOMPARE(window.canvas().controller()->zoom(), 1.0);
            QCOMPARE(window.canvas().controller()->topAnchor().page, 0U);
        }
    }

    void theWindowRemembersItsSizeAndNothingAboutAWindowWithoutSettings() {
        QTemporaryDir dir;
        const QString ini = dir.filePath(QStringLiteral("s.ini"));
        QSize size;
        {
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.resize(777, 555);
            window.show();
            size = window.size();
            window.close();
            QVERIFY(!settings.windowGeometry().isEmpty());
        }
        QSettings backing(ini, QSettings::IniFormat);
        vellora::AppSettings settings(&backing);
        vellora::MainWindow window(&settings);
        window.show();
        QCOMPARE(window.size(), size);
    }
};

QTEST_MAIN(TstTabs)
#include "tst_tabs.moc"