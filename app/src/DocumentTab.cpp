#include "DocumentTab.h"

#include "PasswordDialog.h"
#include "RepairBar.h"

#include <QFileInfo>
#include <QMetaObject>
#include <QVBoxLayout>

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
    // engine repaired the file.
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    m_repairBar = new RepairBar(this);
    m_canvas = new CanvasView(&m_session, this);
    layout->addWidget(m_repairBar);
    layout->addWidget(m_canvas, 1);
    setFocusProxy(m_canvas);

    m_zoomStatus = tr("%1%").arg(qRound(m_canvas->controller()->zoom() * 100.0));

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
    // Qt would delete the canvas after the members, i.e. after the session it draws from.
    delete m_canvas;
}

void DocumentTab::setDocumentStatus(const QString& text) {
    m_documentStatus = text;
    emit statusChanged();
}

bool DocumentTab::open(const QString& path) {
    saveViewState();
    m_canvas->reset();
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

void DocumentTab::saveViewState() {
    if (!m_settings || !m_opened || m_viewKey.isEmpty()) {
        return;
    }
    const CanvasController* controller = m_canvas->controller();
    const PageLayout::Anchor top = controller->topAnchor();
    m_settings->setViewState(m_viewKey, {top.page, top.offsetPoints, controller->zoom()});
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