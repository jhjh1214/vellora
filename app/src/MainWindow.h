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

#include <QHash>
#include <QMainWindow>
#include <QStringList>
#include <functional>
#include <optional>

class QAction;
class QDialog;
class QUrl;
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
    // Help -> Open Log Folder: shows the folder of the application log in the file manager (the
    // folder is created if it does not exist yet). False if the file manager could not be asked.
    bool openLogFolder();
    // What shows a folder; the default asks the desktop (tests replace it).
    using FolderOpener = std::function<bool(const QString& folder)>;
    void setFolderOpener(FolderOpener opener) { m_folderOpener = std::move(opener); }
    // What opens a web address (the issue form of the crash dialog); tests replace it.
    using UrlOpener = std::function<bool(const QUrl& url)>;
    void setUrlOpener(UrlOpener opener) { m_urlOpener = std::move(opener); }

    // If crash reports were written in `directory` (the crash folder if empty) since the user last
    // looked, opens the dialog that offers to show the folder or open the issue form, and returns
    // it. Null when there is nothing new. Whatever the user chooses, these reports are not shown
    // again. Nothing is uploaded.
    QDialog* checkForCrashReports(const QString& directory = {});

    // What the window asks when a document needs a password (all tabs, present and future).
    void setPasswordProvider(PasswordProvider provider);

    // View -> Go to Page: asks for a page number or label and goes there. The default shows a
    // dialog (the current label is its starting text); tests replace it. An empty optional is
    // "cancel".
    using GoToPageProvider =
        std::function<std::optional<QString>(const QString& current, quint32 pageCount)>;
    void setGoToPageProvider(GoToPageProvider provider) {
        m_goToPageProvider = std::move(provider);
    }
    void goToPageDialog();

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
    // A click on the page text in the status bar asks for a page, like Ctrl+G.
    bool eventFilter(QObject* watched, QEvent* event) override;

private slots:
    void chooseDocument();

private:
    DocumentTab* addTab();
    void registerCommands();
    // Check marks of the page layout entries follow the current tab's arrangement.
    void updateViewActions();
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
    GoToPageProvider m_goToPageProvider;
    FolderOpener m_folderOpener;
    UrlOpener m_urlOpener;
    // Used when there are no settings to keep it in.
    QDateTime m_crashSeenUntil;
    QStringList m_closedTabs; // most recent last
    QList<QAction*> m_recentFixedActions;
    QHash<QString, QAction*> m_viewActions;
    // Whether two-page layouts start with a cover page alone on the right.
    bool m_coverInTwoUp = false;
};

} // namespace vellora