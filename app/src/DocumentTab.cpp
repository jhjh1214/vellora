#include "DocumentTab.h"

#include "PasswordDialog.h"
#include "RepairBar.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QFileInfo>
#include <QHBoxLayout>
#include <QMetaObject>
#include <QSplitter>
#include <QVBoxLayout>
#include <algorithm>

namespace vellora {

DocumentTab::DocumentTab(AppSettings* settings, QWidget* parent)
    : QWidget(parent), m_settings(settings) {
    m_passwordProvider = [this](const QString& fileName, int attempt, int maxAttempts, bool wrong) {
        PasswordDialog dialog(fileName, attempt, maxAttempts, wrong, this);
        if (dialog.exec() != QDialog::Accepted) {
            return std::optional<QString>();
        }
        return std::optional<QString>(dialog.takePassword());
    };

    // The repair bar sits above the canvas and takes its height from it; it is hidden unless the
    // engine repaired the file. The thumbnails are on the left, in a splitter, hidden at first.
    auto* layout = new QHBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    m_splitter = new QSplitter(Qt::Horizontal, this);
    m_splitter->setChildrenCollapsible(false);
    auto* content = new QWidget(m_splitter);
    auto* contentLayout = new QVBoxLayout(content);
    contentLayout->setContentsMargins(0, 0, 0, 0);
    contentLayout->setSpacing(0);
    m_repairBar = new RepairBar(content);
    m_canvas = new CanvasView(&m_session, content);
    m_sidebar = new ThumbnailSidebar(&m_session, m_canvas->controller(), m_splitter);
    contentLayout->addWidget(m_repairBar);
    contentLayout->addWidget(m_canvas, 1);
    m_splitter->addWidget(m_sidebar);
    m_splitter->addWidget(content);
    m_splitter->setStretchFactor(0, 0);
    m_splitter->setStretchFactor(1, 1);
    layout->addWidget(m_splitter);
    setFocusProxy(m_canvas);

    const AppSettings::Sidebar sidebar =
        m_settings ? m_settings->sidebar() : AppSettings::Sidebar{};
    m_sidebar->thumbnails().setThumbnailWidth(sidebar.thumbnailWidth);
    m_sidebar->setMinimumWidth(AppSettings::kMinSidebarWidth);
    m_sidebar->setVisible(sidebar.visible);
    const int sidebarWidth =
        sidebar.width > 0 ? sidebar.width : m_sidebar->thumbnails().thumbnailWidth() + 40;
    m_splitter->setSizes({sidebarWidth, 1000});
    connect(m_splitter, &QSplitter::splitterMoved, this, [this] { saveSidebar(); });
    connect(m_sidebar, &ThumbnailSidebar::thumbnailWidthChanged, this, [this] { saveSidebar(); });

    m_zoomStatus = tr("%1%").arg(qRound(m_canvas->controller()->zoom() * 100.0));
    // A new tab arranges its pages the way the user last chose.
    if (m_settings != nullptr) {
        applyLayout(m_settings->lastLayout());
    }
    connect(m_canvas->controller(), &CanvasController::viewModeChanged, this,
            [this](CanvasController::ViewMode) {
                // A layout restored with a document is not the user's new choice.
                if (m_settings != nullptr && !m_applyingLayout) {
                    m_settings->setLastLayout(currentLayout());
                }
                emit viewModeChanged();
            });

    connect(&m_session, &EngineSession::opened, this, &DocumentTab::onOpened);
    connect(&m_session, &EngineSession::requestFailed, this, &DocumentTab::onRequestFailed);
    connect(&m_session, &EngineSession::passwordRequested, this, &DocumentTab::onPasswordRequested);
    connect(&m_session, &EngineSession::engineCrashed, this,
            [this](const QString& how, bool willRestart, const QList<quint64>&) {
                onEngineCrashed(how, willRestart);
            });
    connect(&m_session, &EngineSession::failed, this, &DocumentTab::onFailed);
    connect(&m_session, &EngineSession::engineTimedOut, this, &DocumentTab::onEngineTimedOut);
    connect(&m_session, &EngineSession::documentChanged, this, &DocumentTab::onDocumentChanged);
    connect(m_canvas->controller(), &CanvasController::currentPageChanged, this,
            &DocumentTab::onPageChanged);
    connect(m_canvas->controller(), &CanvasController::zoomChanged, this,
            &DocumentTab::onZoomChanged);
}

DocumentTab::~DocumentTab() {
    // Qt would delete the children after the members, i.e. after the session they draw from.
    delete m_sidebar;
    delete m_canvas;
}

bool DocumentTab::sidebarVisible() const {
    // Not `isVisible`: that is false for a tab that is not the current one.
    return !m_sidebar->isHidden();
}

void DocumentTab::setSidebarVisible(bool visible) {
    if (visible == sidebarVisible()) {
        return;
    }
    m_sidebar->setVisible(visible);
    saveSidebar();
    emit sidebarVisibilityChanged(visible);
}

void DocumentTab::saveSidebar() {
    if (m_settings == nullptr) {
        return;
    }
    AppSettings::Sidebar sidebar = m_settings->sidebar();
    sidebar.visible = sidebarVisible();
    // The width only counts while the sidebar is shown: a hidden one has none.
    if (sidebarVisible() && !m_splitter->sizes().isEmpty()) {
        sidebar.width = m_splitter->sizes().first();
    }
    sidebar.thumbnailWidth = m_sidebar->thumbnails().thumbnailWidth();
    m_settings->setSidebar(sidebar);
}

void DocumentTab::setDocumentStatus(const QString& text) {
    m_documentStatus = text;
    emit statusChanged();
}

bool DocumentTab::open(const QString& path) {
    saveViewState();
    m_canvas->reset();
    m_sidebar->reset();
    m_pageStatus.clear();
    m_fileChanged = false;
    m_opened = false;
    m_wrongPasswords = 0;
    ++m_generation;
    m_repairBar->setReasons({});
    m_documentTip.clear();
    m_path = path;
    m_fileName = QFileInfo(path).fileName();
    const QFileInfo info(path);
    m_viewKey = AppSettings::viewKey(info);
    m_savedView = m_settings ? m_settings->viewState(m_viewKey) : std::nullopt;
    emit titleChanged();

    const QString error = m_session.open(path);
    if (!error.isEmpty()) {
        m_viewKey.clear();
        m_savedView.reset();
        setDocumentStatus(tr("Cannot open: %1").arg(error));
        return false;
    }
    setDocumentStatus(tr("Opening…"));
    return true;
}

AppSettings::Layout DocumentTab::currentLayout() const {
    const CanvasController::ViewMode mode = m_canvas->controller()->viewMode();
    return {mode.continuous, static_cast<int>(mode.spread), mode.rotation};
}

void DocumentTab::applyLayout(const AppSettings::Layout& layout) {
    m_applyingLayout = true;
    m_canvas->controller()->setViewMode(
        {layout.continuous, static_cast<PageLayout::Spread>(std::clamp(layout.spread, 0, 2)),
         layout.rotation});
    m_applyingLayout = false;
}

void DocumentTab::saveViewState() {
    if (!m_settings || !m_opened || m_viewKey.isEmpty()) {
        return;
    }
    const CanvasController* controller = m_canvas->controller();
    const PageLayout::Anchor top = controller->topAnchor();
    m_settings->setViewState(m_viewKey,
                             {top.page, top.offsetPoints, controller->zoom(), currentLayout()});
}

void DocumentTab::onOpened(quint32 pageCount, const QStringList& repairs) {
    // Also the answer of a restarted engine: it has the document again, so tiles are on their way.
    m_wrongPasswords = 0;
    m_canvas->hideBanner();
    emit message(QString());
    if (!m_opened) {
        m_opened = true;
        // Only the first answer: after a restart the view is where the user left it.
        if (m_savedView) {
            applyLayout(m_savedView->layout);
            m_canvas->controller()->restoreView({m_savedView->page, m_savedView->offsetPoints},
                                                m_savedView->zoom);
            m_savedView.reset();
        }
    }
    QString text = tr("%n page(s)", nullptr, static_cast<int>(pageCount));
    if (!repairs.isEmpty()) {
        text += tr(" — repaired");
    }
    // A restarted engine answers again with the same reasons; a bar the user dismissed stays gone.
    if (repairs != m_repairBar->reasons()) {
        m_repairBar->setReasons(repairs);
    }
    if (m_fileChanged) {
        text += tr(" — file changed on disk");
    }
    setDocumentStatus(text);
}

void DocumentTab::onRequestFailed(quint64 request, const QString& message) {
    // A failure that belongs to a request is the canvas's business; one that belongs to no
    // request is the document (for example, the engine could not read the file).
    if (request == 0) {
        setDocumentStatus(tr("Cannot open: %1").arg(message));
    }
}

void DocumentTab::onPasswordRequested(bool wrong) {
    setDocumentStatus(tr("Password required"));
    emit needsAttention();
    // The prompt is modal and runs its own event loop, which must not happen inside the session's
    // event dispatch: it is shown from the event loop instead, for the document that asked.
    const quint64 generation = m_generation;
    QMetaObject::invokeMethod(
        this, [this, wrong, generation] { askForPassword(wrong, generation); },
        Qt::QueuedConnection);
}

void DocumentTab::askForPassword(bool wrong, quint64 generation) {
    if (generation != m_generation || !m_session.isOpen()) {
        return;
    }
    if (wrong) {
        ++m_wrongPasswords;
    }
    if (m_wrongPasswords >= kMaxPasswordAttempts) {
        giveUp(tr("Cannot open: the password was wrong %n time(s)", nullptr, m_wrongPasswords));
        return;
    }
    const std::optional<QString> password = m_passwordProvider(
        m_fileName, m_wrongPasswords + 1, kMaxPasswordAttempts, m_wrongPasswords > 0);
    // The prompt ran its own event loop: the document may have been replaced meanwhile.
    if (generation != m_generation || !m_session.isOpen()) {
        return;
    }
    if (!password) {
        giveUp(tr("Cannot open: a password is required"));
        return;
    }
    const QString error = m_session.submitPassword(*password);
    if (!error.isEmpty()) {
        giveUp(tr("Cannot open: %1").arg(error));
        return;
    }
    setDocumentStatus(tr("Opening…"));
}

void DocumentTab::giveUp(const QString& status) {
    m_session.close();
    m_viewKey.clear();
    setDocumentStatus(status);
    emit titleChanged(); // the window title follows whether a document is open
}

void DocumentTab::onEngineCrashed(const QString& how, bool willRestart) {
    // Non-modal: the window stays usable, and the banner goes away when the engine is back.
    m_canvas->showBanner(willRestart ? tr("Page failed to render — retrying…")
                                     : tr("The page renderer stopped and could not be restarted."));
    emit message(willRestart ? tr("The engine stopped (%1) — restarting").arg(how)
                             : tr("The engine stopped (%1)").arg(how));
}

void DocumentTab::onFailed(const QString& reason) {
    m_canvas->showBanner(tr("The page renderer failed. Reopen the document."));
    setDocumentStatus(tr("Engine failed: %1").arg(reason));
}

void DocumentTab::onEngineTimedOut(TimeoutStage stage, const QString& message) {
    if (stage == TimeoutStage::Tile) {
        // The engine was killed; `engineCrashed` follows and shows the restart.
        emit this->message(tr("The page renderer stopped answering (%1)").arg(message));
        return;
    }
    m_canvas->showBanner(tr("The page renderer did not start in time. Reopen the document."));
    setDocumentStatus(tr("Engine timed out: %1").arg(message));
}

void DocumentTab::onDocumentChanged(bool replaced) {
    // Reported just before the engine restarts over the file, which then shows what is there now.
    m_fileChanged = true;
    m_documentTip =
        replaced ? tr("Another program replaced this file after it was opened. The pages shown "
                      "come from the file as it is now.")
                 : tr("Another program modified this file after it was opened. The pages shown "
                      "come from the file as it is now.");
    emit statusChanged();
}

void DocumentTab::onPageChanged(quint32 page, quint32 pageCount) {
    m_pageStatus = pageCount == 0 ? QString() : tr("Page %1 / %2").arg(page + 1).arg(pageCount);
    emit statusChanged();
}

void DocumentTab::onZoomChanged(double zoom) {
    m_zoomStatus = tr("%1%").arg(qRound(zoom * 100.0));
    emit statusChanged();
}

} // namespace vellora