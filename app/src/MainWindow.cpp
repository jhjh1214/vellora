#include "MainWindow.h"

#include "AboutDialog.h"
#include "CrashDialog.h"
#include "RepairBar.h"
#include "diagnostics/CrashReports.h"
#include "diagnostics/Logging.h"

#include <QAction>
#include <QCloseEvent>
#include <QDesktopServices>
#include <QDir>
#include <QDragEnterEvent>
#include <QDropEvent>
#include <QFileDialog>
#include <QFileInfo>
#include <QLabel>
#include <QMenu>
#include <QMenuBar>
#include <QMimeData>
#include <QScreen>
#include <QSignalBlocker>
#include <QStatusBar>
#include <QStyle>
#include <QTabBar>
#include <QTabWidget>
#include <QUrl>
#include <functional>
#include <utility>

namespace vellora {

namespace {

// Status bar text must never be read as rich text: file names and engine messages are untrusted.
QLabel* plainLabel(QWidget* parent) {
    auto* label = new QLabel(parent);
    label->setTextFormat(Qt::PlainText);
    return label;
}

// Tab titles are file names: a literal `&` must not become a mnemonic.
QString tabTitle(const QString& fileName) {
    QString title = fileName.isEmpty() ? QObject::tr("New Tab") : fileName;
    return title.replace(QLatin1Char('&'), QStringLiteral("&&"));
}

// The command id of the zoom preset (0.25 is "view.zoom.25").
QString zoomPresetId(double preset) {
    return QStringLiteral("view.zoom.%1").arg(qRound(preset * 100.0));
}

// The local files among the URLs of a drop.
QStringList localFiles(const QMimeData* mime) {
    QStringList files;
    if (mime != nullptr) {
        for (const QUrl& url : mime->urls()) {
            if (url.isLocalFile()) {
                files.append(url.toLocalFile());
            }
        }
    }
    return files;
}

} // namespace

MainWindow::MainWindow(AppSettings* settings, QWidget* parent)
    : QMainWindow(parent), m_settings(settings) {
    setWindowTitle(tr("Vellora"));
    setAcceptDrops(true);
    // 1000 x 800, but never more than 80% of the screen.
    const QSize available = screen() ? screen()->availableSize() : QSize(1250, 1000);
    resize(QSize(1000, 800).boundedTo(available * 0.8));
    if (screen()) {
        // Centred, so that the status bar is not below the bottom of a small screen.
        setGeometry(QStyle::alignedRect(Qt::LeftToRight, Qt::AlignCenter, size(),
                                        screen()->availableGeometry()));
    }
    if (m_settings != nullptr && !m_settings->windowGeometry().isEmpty()) {
        // Falls back to the size above if the saved geometry is not usable (a screen is gone).
        restoreGeometry(m_settings->windowGeometry());
    }

    m_tabs = new QTabWidget(this);
    m_tabs->setDocumentMode(true);
    m_tabs->setTabsClosable(true);
    m_tabs->setMovable(true);
    m_tabs->setElideMode(Qt::ElideMiddle);
    m_tabs->tabBar()->setChangeCurrentOnDrag(false);
    setCentralWidget(m_tabs);

    m_documentStatus = plainLabel(this);
    m_pageStatus = plainLabel(this);
    m_zoomStatus = plainLabel(this);
    statusBar()->addPermanentWidget(m_documentStatus);
    statusBar()->addPermanentWidget(m_pageStatus);
    statusBar()->addPermanentWidget(m_zoomStatus);

    registerCommands();
    auto* fileMenu = menuBar()->addMenu(tr("&File"));
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.open"), this));
    QAction* recent = m_commands.createAction(QStringLiteral("file.openRecent"), this);
    m_recentMenu = new QMenu(this);
    recent->setMenu(m_recentMenu);
    fileMenu->addAction(recent);
    connect(m_recentMenu, &QMenu::aboutToShow, this, &MainWindow::refreshRecentMenu);
    fileMenu->addSeparator();
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.close"), this));
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.reopenClosed"), this));
    fileMenu->addSeparator();
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.quit"), this));
    // Shown inside the Open Recent menu.
    for (const char* id : {"file.clearRecent", "file.removeUnavailableRecent"}) {
        m_recentFixedActions.append(m_commands.createAction(QString::fromLatin1(id), this));
    }
    auto* viewMenu = menuBar()->addMenu(tr("&View"));
    const auto addView = [this](QMenu* menu, const char* id, bool checkable = false) {
        QAction* action = m_commands.createAction(QString::fromLatin1(id), this);
        action->setCheckable(checkable);
        menu->addAction(action);
        m_viewActions.insert(QString::fromLatin1(id), action);
    };
    addView(viewMenu, "view.thumbnails", true);
    viewMenu->addSeparator();
    auto* layoutMenu = viewMenu->addMenu(tr("Page &Layout"));
    for (const char* id : {"view.layout.single", "view.layout.continuous", "view.layout.twoUp",
                           "view.layout.twoUpContinuous"}) {
        addView(layoutMenu, id, true);
    }
    layoutMenu->addSeparator();
    addView(layoutMenu, "view.layout.cover", true);
    auto* zoomMenu = viewMenu->addMenu(tr("&Zoom"));
    for (const char* id :
         {"view.zoomIn", "view.zoomOut", "view.actualSize", "view.fitWidth", "view.fitPage"}) {
        addView(zoomMenu, id);
    }
    zoomMenu->addSeparator();
    for (const double preset : CanvasController::zoomPresets()) {
        addView(zoomMenu, qPrintable(zoomPresetId(preset)));
    }
    viewMenu->addSeparator();
    addView(viewMenu, "view.rotateClockwise");
    addView(viewMenu, "view.rotateCounterclockwise");
    addView(viewMenu, "view.zoomToSelection");
    viewMenu->addSeparator();
    addView(viewMenu, "view.nextPage");
    addView(viewMenu, "view.previousPage");
    viewMenu->addSeparator();
    viewMenu->addAction(m_commands.createAction(QStringLiteral("palette.show"), this));
    auto* windowMenu = menuBar()->addMenu(tr("&Window"));
    windowMenu->addAction(m_commands.createAction(QStringLiteral("tabs.next"), this));
    windowMenu->addAction(m_commands.createAction(QStringLiteral("tabs.previous"), this));

    auto* helpMenu = menuBar()->addMenu(tr("&Help"));
    helpMenu->addAction(m_commands.createAction(QStringLiteral("help.openLogFolder"), this));
    helpMenu->addAction(m_commands.createAction(QStringLiteral("help.about"), this));

    m_palette = new CommandPalette(&m_commands, this);

    connect(m_tabs, &QTabWidget::currentChanged, this, [this] { updateChrome(); });
    connect(m_tabs, &QTabWidget::tabCloseRequested, this, &MainWindow::closeTab);
    addTab();
}

void MainWindow::registerCommands() {
    // Every action of the window is one of these; menus, shortcuts and the palette derive from
    // them.
    const auto add = [this](const char* id, const QString& title, QList<QKeySequence> shortcuts,
                            std::function<void()> handler) {
        const bool added = m_commands.add(
            {QString::fromLatin1(id), title, std::move(shortcuts), std::move(handler)});
        Q_ASSERT(added);
        Q_UNUSED(added);
    };
    add("file.open", tr("Open…"), {QKeySequence::Open}, [this] { chooseDocument(); });
    add("file.openRecent", tr("Open Recent"), {}, [this] {
        refreshRecentMenu();
        m_recentMenu->popup(mapToGlobal(rect().center()));
    });
    add("file.clearRecent", tr("Clear Recent Files"), {}, [this] {
        if (m_settings != nullptr) {
            m_settings->clearRecentFiles();
        }
    });
    add("file.removeUnavailableRecent", tr("Remove Unavailable Files"), {},
        [this] { removeUnavailableRecent(); });
    add("file.close", tr("Close Tab"), {QKeySequence::Close}, [this] { closeCurrentTab(); });
    add("file.reopenClosed", tr("Reopen Closed Tab"),
        {QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_T)}, [this] { reopenClosedTab(); });
    add("file.quit", tr("Quit"), QKeySequence::keyBindings(QKeySequence::Quit),
        [this] { close(); });
    add("tabs.next", tr("Next Tab"), {QKeySequence(Qt::CTRL | Qt::Key_Tab)}, [this] {
        if (tabCount() > 1) {
            setCurrentTabIndex((currentTabIndex() + 1) % tabCount());
        }
    });
    add("tabs.previous", tr("Previous Tab"), {QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_Tab)},
        [this] {
            if (tabCount() > 1) {
                setCurrentTabIndex((currentTabIndex() + tabCount() - 1) % tabCount());
            }
        });
    add("view.thumbnails", tr("Page Thumbnails"), {QKeySequence(Qt::Key_F4)}, [this] {
        currentTab().setSidebarVisible(!currentTab().sidebarVisible());
        updateViewActions();
    });
    add("view.zoomIn", tr("Zoom In"),
        {QKeySequence(QKeySequence::ZoomIn), QKeySequence(Qt::CTRL | Qt::Key_Equal)},
        [this] { canvas().zoomIn(); });
    add("view.zoomOut", tr("Zoom Out"), {QKeySequence::ZoomOut}, [this] { canvas().zoomOut(); });
    add("view.actualSize", tr("Actual Size"), {QKeySequence(Qt::CTRL | Qt::Key_1)},
        [this] { canvas().actualSize(); });
    add("view.fitWidth", tr("Fit Width"), {QKeySequence(Qt::CTRL | Qt::Key_2)},
        [this] { canvas().fitWidth(); });
    add("view.fitPage", tr("Fit Page"), {QKeySequence(Qt::CTRL | Qt::Key_0)},
        [this] { canvas().fitPage(); });
    for (const double preset : CanvasController::zoomPresets()) {
        const QByteArray id = zoomPresetId(preset).toLatin1();
        add(id.constData(), tr("Zoom %1%").arg(qRound(preset * 100.0)), {}, [this, preset] {
            CanvasView& view = canvas();
            view.controller()->setZoom(
                preset, QPointF(view.viewport()->width() / 2.0, view.viewport()->height() / 2.0));
        });
    }
    // Page layout: one command each, so that the menu, the palette and shortcuts agree.
    const auto setLayout = [this](bool continuous, bool twoUp) {
        const PageLayout::Spread spread =
            !twoUp ? PageLayout::Spread::One
                   : (m_coverInTwoUp ? PageLayout::Spread::TwoCover : PageLayout::Spread::Two);
        CanvasController::ViewMode mode = canvas().controller()->viewMode();
        mode.continuous = continuous;
        mode.spread = spread;
        canvas().controller()->setViewMode(mode);
        updateViewActions();
    };
    add("view.layout.single", tr("Single Page"), {}, [setLayout] { setLayout(false, false); });
    add("view.layout.continuous", tr("Continuous Scrolling"), {},
        [setLayout] { setLayout(true, false); });
    add("view.layout.twoUp", tr("Two Pages"), {}, [setLayout] { setLayout(false, true); });
    add("view.layout.twoUpContinuous", tr("Two Pages, Continuous"), {},
        [setLayout] { setLayout(true, true); });
    add("view.layout.cover", tr("Cover Page in Two-Page View"), {}, [this] {
        m_coverInTwoUp = !m_coverInTwoUp;
        CanvasController* controller = canvas().controller();
        if (controller->viewMode().spread != PageLayout::Spread::One) {
            controller->setSpread(m_coverInTwoUp ? PageLayout::Spread::TwoCover
                                                 : PageLayout::Spread::Two);
        }
        updateViewActions();
    });
    add("view.rotateClockwise", tr("Rotate Clockwise"),
        {QKeySequence(Qt::CTRL | Qt::Key_BracketRight)},
        [this] { canvas().controller()->rotateBy(1); });
    add("view.rotateCounterclockwise", tr("Rotate Counterclockwise"),
        {QKeySequence(Qt::CTRL | Qt::Key_BracketLeft)},
        [this] { canvas().controller()->rotateBy(-1); });
    add("view.zoomToSelection", tr("Zoom to Selection"), {},
        [this] { canvas().armZoomRect(true); });
    add("view.nextPage", tr("Next Page"), {QKeySequence(Qt::CTRL | Qt::Key_Down)},
        [this] { canvas().controller()->nextPage(); });
    add("view.previousPage", tr("Previous Page"), {QKeySequence(Qt::CTRL | Qt::Key_Up)},
        [this] { canvas().controller()->previousPage(); });
    add("help.openLogFolder", tr("Open Log Folder"), {}, [this] { openLogFolder(); });
    add("help.about", tr("About Vellora"), {}, [this] { showAbout(); });
    add("palette.show", tr("Command Palette…"), {QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_P)},
        [this] { m_palette->open(); });
}

MainWindow::~MainWindow() {
    // Each tab ends its engine; do it before the members they report to go away.
    while (m_tabs->count() > 0) {
        delete m_tabs->widget(0);
    }
}

DocumentTab* MainWindow::addTab() {
    auto* tab = new DocumentTab(m_settings, m_tabs);
    if (m_passwordProvider) {
        tab->setPasswordProvider(m_passwordProvider);
    }
    const int index = m_tabs->addTab(tab, tabTitle(QString()));
    connect(tab, &DocumentTab::statusChanged, this, [this, tab] {
        if (m_tabs->currentWidget() == tab) {
            updateChrome();
        }
    });
    connect(tab, &DocumentTab::titleChanged, this, [this, tab] { updateTabTitle(tab); });
    connect(tab, &DocumentTab::viewModeChanged, this, [this, tab] {
        if (m_tabs->currentWidget() == tab) {
            updateViewActions();
        }
    });
    connect(tab, &DocumentTab::sidebarVisibilityChanged, this, [this, tab] {
        if (m_tabs->currentWidget() == tab) {
            updateViewActions();
        }
    });
    connect(tab, &DocumentTab::message, this, [this, tab](const QString& text) {
        if (m_tabs->currentWidget() == tab) {
            if (text.isEmpty()) {
                statusBar()->clearMessage();
            } else {
                statusBar()->showMessage(text);
            }
        }
    });
    connect(tab, &DocumentTab::needsAttention, this,
            [this, tab] { m_tabs->setCurrentWidget(tab); });
    m_tabs->setCurrentIndex(index);
    emit tabAdded(tab);
    updateChrome();
    return tab;
}

void MainWindow::updateViewActions() {
    if (m_tabs->currentWidget() == nullptr) {
        return;
    }
    const CanvasController::ViewMode mode = currentTab().canvas().controller()->viewMode();
    const bool twoUp = mode.spread != PageLayout::Spread::One;
    if (twoUp) {
        m_coverInTwoUp = mode.spread == PageLayout::Spread::TwoCover;
    }
    const auto check = [this](const char* id, bool on) {
        if (QAction* action = m_viewActions.value(QString::fromLatin1(id))) {
            const QSignalBlocker blocker(action);
            action->setChecked(on);
        }
    };
    check("view.layout.single", !mode.continuous && !twoUp);
    check("view.layout.continuous", mode.continuous && !twoUp);
    check("view.layout.twoUp", !mode.continuous && twoUp);
    check("view.layout.twoUpContinuous", mode.continuous && twoUp);
    check("view.layout.cover", m_coverInTwoUp);
    check("view.thumbnails", currentTab().sidebarVisible());
}

void MainWindow::updateTabTitle(DocumentTab* tab) {
    const int index = m_tabs->indexOf(tab);
    if (index >= 0) {
        m_tabs->setTabText(index, tabTitle(tab->fileName()));
        m_tabs->setTabToolTip(index, tab->path());
    }
    updateChrome();
}

void MainWindow::updateChrome() {
    // No current tab while the last one is being removed (or the window is being destroyed).
    if (m_tabs->currentWidget() == nullptr) {
        return;
    }
    DocumentTab& tab = currentTab();
    m_documentStatus->setText(tab.documentStatus());
    m_documentStatus->setToolTip(tab.documentStatusTip());
    m_pageStatus->setText(tab.pageStatus());
    m_zoomStatus->setText(tab.zoomStatus());
    statusBar()->clearMessage();
    updateViewActions();
    setWindowTitle(!tab.isEmpty() && tab.session().isOpen() ? tr("%1 — Vellora").arg(tab.fileName())
                                                            : tr("Vellora"));
}

int MainWindow::tabCount() const {
    return m_tabs->count();
}

DocumentTab& MainWindow::tab(int index) {
    return *qobject_cast<DocumentTab*>(m_tabs->widget(index));
}

DocumentTab& MainWindow::currentTab() {
    return *qobject_cast<DocumentTab*>(m_tabs->currentWidget());
}

int MainWindow::currentTabIndex() const {
    return m_tabs->currentIndex();
}

void MainWindow::setCurrentTabIndex(int index) {
    m_tabs->setCurrentIndex(index);
}

QDialog* MainWindow::showAbout() {
    auto* dialog = new AboutDialog(this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    dialog->open();
    return dialog;
}

bool MainWindow::openLogFolder() {
    const QString folder = Logging::directory();
    // Exists even when logging could not start, so that there is something to show.
    QDir().mkpath(folder);
    if (m_folderOpener) {
        return m_folderOpener(folder);
    }
    return QDesktopServices::openUrl(QUrl::fromLocalFile(folder));
}

QDialog* MainWindow::checkForCrashReports(const QString& directory) {
    const QString folder = directory.isEmpty() ? CrashReports::directory() : directory;
    const QDateTime seen =
        m_settings != nullptr ? m_settings->crashReportsSeenUntil() : m_crashSeenUntil;
    const QList<CrashReports::Report> reports = CrashReports::pending(folder, seen);
    if (reports.isEmpty()) {
        return nullptr;
    }
    // The newest report is the mark: later ones are new, these are not shown again.
    const QDateTime newest = reports.first().modified;
    auto* dialog = new CrashDialog(reports, this);
    dialog->setAttribute(Qt::WA_DeleteOnClose);
    connect(dialog, &CrashDialog::chosen, this,
            [this, folder, reports, newest](CrashDialog::Choice choice) {
                if (m_settings != nullptr) {
                    m_settings->setCrashReportsSeenUntil(newest);
                    m_settings->sync();
                } else {
                    m_crashSeenUntil = newest;
                }
                if (choice == CrashDialog::Choice::OpenFolder) {
                    if (m_folderOpener) {
                        m_folderOpener(folder);
                    } else {
                        QDesktopServices::openUrl(QUrl::fromLocalFile(folder));
                    }
                } else if (choice == CrashDialog::Choice::Report) {
                    const QUrl url = CrashReports::issueUrl(reports);
                    if (m_urlOpener) {
                        m_urlOpener(url);
                    } else {
                        QDesktopServices::openUrl(url);
                    }
                }
            });
    dialog->open();
    return dialog;
}

void MainWindow::setPasswordProvider(PasswordProvider provider) {
    m_passwordProvider = std::move(provider);
    for (int i = 0; i < m_tabs->count(); ++i) {
        tab(i).setPasswordProvider(m_passwordProvider);
    }
}

QString MainWindow::documentStatus() const {
    return m_documentStatus->text();
}

QString MainWindow::pageStatus() const {
    return m_pageStatus->text();
}

QString MainWindow::zoomStatus() const {
    return m_zoomStatus->text();
}

void MainWindow::chooseDocument() {
    const QString start = m_settings != nullptr ? m_settings->lastDirectory() : QString();
    const QStringList paths =
        QFileDialog::getOpenFileNames(this, tr("Open PDF"), start, tr("PDF documents (*.pdf)"));
    openDocuments(paths);
}

bool MainWindow::openDocument(const QString& path) {
    const QString normal = normalizedPath(path);
    for (int i = 0; i < tabCount(); ++i) {
        if (!tab(i).isEmpty() && samePath(tab(i).path(), normal)) {
            setCurrentTabIndex(i);
            return true;
        }
    }
    DocumentTab* target = currentTab().isEmpty() ? &currentTab() : addTab();
    const bool ok = target->open(normal);
    if (ok && m_settings != nullptr) {
        m_settings->addRecentFile(normal);
        m_settings->setLastDirectory(QFileInfo(normal).absolutePath());
    }
    return ok;
}

QStringList MainWindow::openDocuments(const QStringList& paths) {
    QStringList failed;
    for (const QString& path : paths) {
        if (!openDocument(path)) {
            failed.append(path);
        }
    }
    return failed;
}

void MainWindow::closeTab(int index) {
    if (index < 0 || index >= tabCount()) {
        return;
    }
    DocumentTab* closing = &tab(index);
    closing->saveViewState();
    if (!closing->isEmpty()) {
        m_closedTabs.append(closing->path());
        while (m_closedTabs.size() > kMaxClosedTabs) {
            m_closedTabs.removeFirst();
        }
    }
    m_tabs->removeTab(index);
    // Ends the tab's engine process.
    delete closing;
    if (m_tabs->count() == 0) {
        addTab();
    }
    if (m_settings != nullptr) {
        m_settings->sync();
    }
    updateChrome();
}

bool MainWindow::reopenClosedTab() {
    while (!m_closedTabs.isEmpty()) {
        const QString path = m_closedTabs.takeLast();
        // A file that is gone is skipped: the next one in the list is the closest to what was
        // meant.
        if (QFileInfo(path).isReadable()) {
            return openDocument(path);
        }
    }
    return false;
}

void MainWindow::refreshRecentMenu() {
    m_recentMenu->clear();
    const QStringList files = m_settings != nullptr ? m_settings->recentFiles() : QStringList();
    for (const QString& path : files) {
        const QFileInfo info(path);
        QAction* entry = m_recentMenu->addAction(tabTitle(info.fileName()));
        entry->setToolTip(path);
        entry->setStatusTip(path);
        if (info.isReadable()) {
            connect(entry, &QAction::triggered, this, [this, path] { openDocument(path); });
        } else {
            // Greyed out: the file is gone, moved or not readable. "Remove Unavailable Files"
            // drops them from the list.
            entry->setEnabled(false);
        }
    }
    if (!files.isEmpty()) {
        m_recentMenu->addSeparator();
    }
    for (QAction* action : std::as_const(m_recentFixedActions)) {
        m_recentMenu->addAction(action);
    }
}

void MainWindow::removeUnavailableRecent() {
    if (m_settings == nullptr) {
        return;
    }
    for (const QString& path : m_settings->recentFiles()) {
        if (!QFileInfo(path).isReadable()) {
            m_settings->removeRecentFile(path);
        }
    }
}

void MainWindow::closeEvent(QCloseEvent* event) {
    for (int i = 0; i < tabCount(); ++i) {
        tab(i).saveViewState();
    }
    if (m_settings != nullptr) {
        m_settings->setWindowGeometry(saveGeometry());
        m_settings->sync();
    }
    QMainWindow::closeEvent(event);
}

void MainWindow::dragEnterEvent(QDragEnterEvent* event) {
    if (!localFiles(event->mimeData()).isEmpty()) {
        event->acceptProposedAction();
    }
}

void MainWindow::dropEvent(QDropEvent* event) {
    const QStringList files = localFiles(event->mimeData());
    if (files.isEmpty()) {
        return;
    }
    event->acceptProposedAction();
    openDocuments(files);
}

} // namespace vellora