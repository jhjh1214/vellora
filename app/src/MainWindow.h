// The main window: native menu bar (File, View, Window), one tab per open document, and a status
// bar with the current tab's page, zoom and state. Every action is a registered command (the menus,
// the shortcuts and the command palette derive from the registry).
//
// Each tab (`DocumentTab`) has its own engine process, which ends when the tab closes. The window
// always has at least one tab; a tab without a document is "empty" and is reused by the next file
// that is opened. The accessors without a tab argument (`session()`, `canvas()`, ...) are the
// current tab's.
#pragma once

#include "DocumentTab.h"
#include "bridge/EngineSession.h"
#include "canvas/CanvasView.h"
#include "commands/CommandPalette.h"
#include "commands/CommandRegistry.h"
#include "settings/AppSettings.h"

#include <QMainWindow>
#include <QStringList>

class QDialog;
class QLabel;
class QMenu;
class QTabWidget;

namespace vellora {

class RepairBar;

class MainWindow : public QMainWindow {
    Q_OBJECT

public:
    // How many wrong passwords end the attempt to open an encrypted document.
    static constexpr int kMaxPasswordAttempts = DocumentTab::kMaxPasswordAttempts;
    // How many closed tabs "Reopen Closed Tab" remembers.
    static constexpr int kMaxClosedTabs = 20;
    using PasswordProvider = DocumentTab::PasswordProvider;

    // `settings` is not owned and may be null: then nothing is remembered between runs.
    explicit MainWindow(AppSettings* settings = nullptr, QWidget* parent = nullptr);
    ~MainWindow() override;

    // Opens `path` in a tab: the tab that already shows it, else the current tab if it is empty,
    // else a new one. False (the reason is in the tab's `documentStatus`) if the engine cannot be
    // started.
    bool openDocument(const QString& path);
    // Opens each of `paths` (a command line, a drop, another launch). Returns the ones that could
    // not be opened.
    QStringList openDocuments(const QStringList& paths);

    int tabCount() const;
    DocumentTab& tab(int index);
    DocumentTab& currentTab();
    int currentTabIndex() const;
    void setCurrentTabIndex(int index);
    // Closes the tab (its engine ends with it); the window keeps at least one, empty, tab.
    void closeTab(int index);
    void closeCurrentTab() { closeTab(currentTabIndex()); }
    // Opens the file of the tab closed last, if any (up to `kMaxClosedTabs`). False if none.
    bool reopenClosedTab();
    QStringList closedTabs() const { return m_closedTabs; }

    // File -> Open Recent. Rebuilt every time it is shown; unreadable files are greyed out.
    QMenu* recentMenu() { return m_recentMenu; }
    void refreshRecentMenu();

    // Help -> About Vellora. Opens the dialog without blocking and returns it.
    QDialog* showAbout();

    // What the window asks when a document needs a password (all tabs, present and future).
    void setPasswordProvider(PasswordProvider provider);

    // The current tab's parts and status texts.
    EngineSession& session() { return currentTab().session(); }
    CanvasView& canvas() { return currentTab().canvas(); }
    RepairBar& repairBar() { return currentTab().repairBar(); }
    CommandRegistry& commands() { return m_commands; }
    CommandPalette& palette() { return *m_palette; }
    QString documentStatus() const;
    QString pageStatus() const;
    QString zoomStatus() const;

signals:
    // A tab was created (the watchdog wants to time its canvas).
    void tabAdded(vellora::DocumentTab* tab);

protected:
    void closeEvent(QCloseEvent* event) override;
    void dragEnterEvent(QDragEnterEvent* event) override;
    void dropEvent(QDropEvent* event) override;

private slots:
    void chooseDocument();

private:
    DocumentTab* addTab();
    void registerCommands();
    void updateChrome();
    void updateTabTitle(DocumentTab* tab);
    void removeUnavailableRecent();

    AppSettings* m_settings;
    CommandRegistry m_commands;
    QTabWidget* m_tabs = nullptr;
    CommandPalette* m_palette = nullptr;
    QMenu* m_recentMenu = nullptr;
    QLabel* m_documentStatus = nullptr;
    QLabel* m_pageStatus = nullptr;
    QLabel* m_zoomStatus = nullptr;
    PasswordProvider m_passwordProvider;
    QStringList m_closedTabs; // most recent last
    QList<QAction*> m_recentFixedActions;
};

} // namespace vellora