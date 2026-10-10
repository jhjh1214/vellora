#include "DocumentTab.h"

#include "PasswordDialog.h"
#include "RepairBar.h"
#include "links/LinkActions.h"
#include "links/UriConfirmDialog.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QDesktopServices>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QMessageBox>
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

    m_uriConfirmer = [this](const QString& uri, const QString& host) {
        UriConfirmDialog dialog(uri, host, this);
        const bool open = dialog.exec() == QDialog::Accepted;
        return UriChoice{open, open && dialog.trustHost()};
    };
    m_uriOpener = [](const QUrl& url) { return QDesktopServices::openUrl(url); };
    m_notifier = [this](const QString& text) {
        QMessageBox box(QMessageBox::Information, tr("Link"), text, QMessageBox::Ok, this);
        box.setTextFormat(Qt::PlainText); // the text includes the document's own words
        box.exec();
    };
    m_selectAllConfirmer = [this](quint32 pageCount) {
        return QMessageBox::question(this, tr("Select All"),
                                     tr("This document has %n page(s). Selecting all of its text "
                                        "means reading all of "
                                        "it when you copy.\n\nSelect it anyway?",
                                        nullptr, static_cast<int>(pageCount))) == QMessageBox::Yes;
    };
    connect(m_canvas, &CanvasView::linkActivated, this, &DocumentTab::activateLink);
    connect(m_canvas, &CanvasView::copied, this, [this](int characters) {
        emit message(tr("Copied %n character(s)", nullptr, characters));
    });
    m_canvas->setLinkDescriber([this](const Link& link) {
        return LinkActions::describe(link, [this](quint32 page) { return pageLabel(page); });
    });

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
    connect(&m_session, &EngineSession::pageLabelsReady, this, &DocumentTab::onPageLabels);
    connect(&m_session, &EngineSession::pageFound, this, &DocumentTab::onPageFound);
    connect(&m_history, &NavigationHistory::changed, this, &DocumentTab::historyChanged);
    connect(&m_sidebar->outline(), &OutlineView::destinationActivated, this, &DocumentTab::jumpTo);
    connect(&m_sidebar->thumbnails(), &ThumbnailView::aboutToJump, this,
            [this] { m_history.recordJump(currentPlace()); });
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
    m_history.clear();
    m_trustedHosts.clear();
    m_canvas->links()->reset();
    m_labels.clear();
    m_labelsDefined = false;
    m_labelRequest = 0;
    m_findRequest = 0;
    m_page = 0;
    m_pageCount = 0;
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
        requestLabels(0);
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
    if (request != 0 && request == m_labelRequest) {
        m_labelRequest = 0; // the labels could not be read: pages keep their numbers
        return;
    }
    if (request != 0 && request == m_findRequest) {
        m_findRequest = 0;
        goToPageNumber(m_findText);
        return;
    }
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
    m_page = page;
    m_pageCount = pageCount;
    refreshPageStatus();
}

void DocumentTab::refreshPageStatus() {
    if (m_pageCount == 0) {
        m_pageStatus.clear();
    } else if (m_labelsDefined && pageLabel(m_page) != QString::number(m_page + 1)) {
        m_pageStatus =
            tr("Page %1 (%2 / %3)").arg(pageLabel(m_page)).arg(m_page + 1).arg(m_pageCount);
    } else {
        m_pageStatus = tr("Page %1 / %2").arg(m_page + 1).arg(m_pageCount);
    }
    emit statusChanged();
}

QString DocumentTab::pageLabel(quint32 page) const {
    return m_sidebar->thumbnails().labelFor(page);
}

// ---- page labels ----

void DocumentTab::requestLabels(quint32 first) {
    m_labelRequest = m_session.requestPageLabels(first, kLabelsPerRequest);
}

void DocumentTab::onPageLabels(quint64 request, quint32 first, bool defined,
                               const QStringList& labels) {
    if (request == 0 || request != m_labelRequest) {
        return;
    }
    m_labelRequest = 0;
    if (!defined) {
        return; // the labels are the page numbers, which is what the sidebar shows anyway
    }
    // The windows arrive in order; a window that does not continue the list is not trusted.
    if (static_cast<quint32>(m_labels.size()) != first) {
        return;
    }
    m_labels.append(labels);
    m_labelsDefined = true;
    const quint32 next = first + static_cast<quint32>(labels.size());
    if (labels.size() == static_cast<qsizetype>(kLabelsPerRequest) && next < kMaxLabelledPages) {
        requestLabels(next);
        return;
    }
    m_sidebar->thumbnails().setPageLabels(m_labels);
    refreshPageStatus();
}

// ---- jumps ----

NavigationHistory::Place DocumentTab::currentPlace() const {
    const CanvasController* controller = m_canvas->controller();
    const PageLayout::Anchor top = controller->topAnchor();
    return {top.page, top.offsetPoints, controller->zoom()};
}

void DocumentTab::jumpTo(const Destination& destination) {
    m_history.recordJump(currentPlace());
    m_canvas->controller()->goToDestination(destination);
}

void DocumentTab::jumpToPage(quint32 page) {
    m_history.recordJump(currentPlace());
    m_canvas->controller()->goToPage(page);
}

bool DocumentTab::goBack() {
    const auto place = m_history.back(currentPlace());
    if (!place) {
        return false;
    }
    m_canvas->controller()->restoreView({place->page, place->offsetPoints}, place->zoom);
    return true;
}

bool DocumentTab::goForward() {
    const auto place = m_history.forward(currentPlace());
    if (!place) {
        return false;
    }
    m_canvas->controller()->restoreView({place->page, place->offsetPoints}, place->zoom);
    return true;
}

void DocumentTab::goToPageText(const QString& text) {
    const QString wanted = text.trimmed();
    if (wanted.isEmpty() || m_pageCount == 0) {
        return;
    }
    m_findText = wanted;
    // The engine knows the labels, including beyond what has been read here.
    m_findRequest = m_session.findPageLabel(wanted);
    if (m_findRequest == 0) {
        goToPageNumber(wanted);
    }
}

void DocumentTab::onPageFound(quint64 request, bool found, quint32 page) {
    if (request == 0 || request != m_findRequest) {
        return;
    }
    m_findRequest = 0;
    if (found) {
        jumpToPage(page);
    } else {
        goToPageNumber(m_findText);
    }
}

void DocumentTab::goToPageNumber(const QString& text) {
    bool ok = false;
    const quint32 number = text.toUInt(&ok);
    if (ok && number >= 1 && number <= m_pageCount) {
        jumpToPage(number - 1);
        return;
    }
    emit message(tr("This document has no page \"%1\".").arg(text));
}

void DocumentTab::activateLink(const Link& link) {
    switch (link.kind) {
    case LinkKind::GoTo:
        jumpTo(link.destination);
        break;
    case LinkKind::Named: {
        const quint32 count = m_canvas->controller()->pageCount();
        const quint32 current = m_canvas->controller()->currentPage();
        if (count == 0) {
            break;
        }
        switch (link.named) {
        case NamedKind::NextPage:
            if (current + 1 < count) {
                jumpToPage(current + 1);
            }
            break;
        case NamedKind::PrevPage:
            if (current > 0) {
                jumpToPage(current - 1);
            }
            break;
        case NamedKind::FirstPage:
            jumpToPage(0);
            break;
        case NamedKind::LastPage:
            jumpToPage(count - 1);
            break;
        default:
            break;
        }
        break;
    }
    case LinkKind::Uri:
        openUri(link.text);
        break;
    case LinkKind::Inert:
        notify(LinkActions::inertNotice(link.text));
        break;
    default:
        break; // a link that goes nowhere
    }
}

void DocumentTab::openUri(const QString& uri) {
    const LinkActions::UriCheck check = LinkActions::checkUri(uri);
    if (!check.allowed) {
        notify(tr("This address was not opened. %1\n\n%2").arg(check.problem, uri));
        return;
    }
    const bool trusted = !check.host.isEmpty() && m_trustedHosts.contains(check.host);
    if (!trusted) {
        const UriChoice choice = m_uriConfirmer(uri, check.host);
        if (!choice.open) {
            return;
        }
        if (choice.trustHost && !check.host.isEmpty()) {
            m_trustedHosts.insert(check.host);
        }
    }
    if (!m_uriOpener(QUrl(uri, QUrl::StrictMode))) {
        notify(tr("The address could not be opened.\n\n%1").arg(uri));
    }
}

void DocumentTab::notify(const QString& text) {
    emit message(text.section(QLatin1Char('\n'), 0, 0));
    m_notifier(text);
}

void DocumentTab::selectAll() {
    const quint32 count = m_canvas->controller()->pageCount();
    if (count == 0) {
        return;
    }
    if (count > kSelectAllConfirmPages && !m_selectAllConfirmer(count)) {
        return;
    }
    m_canvas->selection()->selectAll(count);
}

void DocumentTab::selectPage() {
    if (m_canvas->controller()->pageCount() != 0) {
        m_canvas->selection()->selectPage(m_canvas->controller()->currentPage());
    }
}

void DocumentTab::copy() {
    m_canvas->copySelection();
}

void DocumentTab::showOutline() {
    setSidebarVisible(true);
    m_sidebar->setCurrentTab(ThumbnailSidebar::Tab::Outline);
}

void DocumentTab::onZoomChanged(double zoom) {
    m_zoomStatus = tr("%1%").arg(qRound(zoom * 100.0));
    emit statusChanged();
}

} // namespace vellora