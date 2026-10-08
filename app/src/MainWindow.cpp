#include "MainWindow.h"

#include "PasswordDialog.h"
#include "RepairBar.h"

#include <QAction>
#include <QFileDialog>
#include <QFileInfo>
#include <QLabel>
#include <QMenuBar>
#include <QMetaObject>
#include <QScreen>
#include <QStatusBar>
#include <QStyle>
#include <QVBoxLayout>
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

} // namespace

MainWindow::MainWindow(QWidget* parent) : QMainWindow(parent) {
    setWindowTitle(tr("Vellora"));
    m_passwordProvider = [this](const QString& fileName, int attempt, int maxAttempts, bool wrong) {
        PasswordDialog dialog(fileName, attempt, maxAttempts, wrong, this);
        if (dialog.exec() != QDialog::Accepted) {
            return std::optional<QString>();
        }
        return std::optional<QString>(dialog.takePassword());
    };
    // 1000 x 800, but never more than 80% of the screen.
    const QSize available = screen() ? screen()->availableSize() : QSize(1250, 1000);
    resize(QSize(1000, 800).boundedTo(available * 0.8));
    if (screen()) {
        // Centred, so that the status bar is not below the bottom of a small screen.
        setGeometry(QStyle::alignedRect(Qt::LeftToRight, Qt::AlignCenter, size(),
                                        screen()->availableGeometry()));
    }

    // The repair bar sits above the canvas and takes its height from it; it is hidden unless the
    // engine repaired the file.
    auto* central = new QWidget(this);
    auto* centralLayout = new QVBoxLayout(central);
    centralLayout->setContentsMargins(0, 0, 0, 0);
    centralLayout->setSpacing(0);
    m_repairBar = new RepairBar(central);
    m_canvas = new CanvasView(&m_session, central);
    centralLayout->addWidget(m_repairBar);
    centralLayout->addWidget(m_canvas, 1);
    setCentralWidget(central);

    m_documentStatus = plainLabel(this);
    m_pageStatus = plainLabel(this);
    m_zoomStatus = plainLabel(this);
    statusBar()->addPermanentWidget(m_documentStatus);
    statusBar()->addPermanentWidget(m_pageStatus);
    statusBar()->addPermanentWidget(m_zoomStatus);
    onZoomChanged(m_canvas->controller()->zoom());

    registerCommands();
    auto* fileMenu = menuBar()->addMenu(tr("&File"));
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.open"), this));
    fileMenu->addSeparator();
    fileMenu->addAction(m_commands.createAction(QStringLiteral("file.quit"), this));
    auto* viewMenu = menuBar()->addMenu(tr("&View"));
    for (const char* id : {"view.zoomIn", "view.zoomOut", "view.actualSize", "view.fitWidth"}) {
        viewMenu->addAction(m_commands.createAction(QString::fromLatin1(id), this));
    }
    viewMenu->addSeparator();
    viewMenu->addAction(m_commands.createAction(QStringLiteral("palette.show"), this));

    m_palette = new CommandPalette(&m_commands, this);

    connect(&m_session, &EngineSession::opened, this, &MainWindow::onOpened);
    connect(&m_session, &EngineSession::requestFailed, this, &MainWindow::onRequestFailed);
    connect(&m_session, &EngineSession::passwordRequested, this, &MainWindow::onPasswordRequested);
    connect(&m_session, &EngineSession::engineCrashed, this,
            [this](const QString& how, bool willRestart, const QList<quint64>&) {
                onEngineCrashed(how, willRestart);
            });
    connect(&m_session, &EngineSession::failed, this, &MainWindow::onFailed);
    connect(&m_session, &EngineSession::engineTimedOut, this, &MainWindow::onEngineTimedOut);
    connect(&m_session, &EngineSession::documentChanged, this, &MainWindow::onDocumentChanged);
    connect(m_canvas->controller(), &CanvasController::currentPageChanged, this,
            &MainWindow::onPageChanged);
    connect(m_canvas->controller(), &CanvasController::zoomChanged, this,
            &MainWindow::onZoomChanged);
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
    add("file.quit", tr("Quit"), QKeySequence::keyBindings(QKeySequence::Quit),
        [this] { close(); });
    add("view.zoomIn", tr("Zoom In"),
        {QKeySequence(QKeySequence::ZoomIn), QKeySequence(Qt::CTRL | Qt::Key_Equal)},
        [this] { m_canvas->zoomIn(); });
    add("view.zoomOut", tr("Zoom Out"), {QKeySequence::ZoomOut}, [this] { m_canvas->zoomOut(); });
    add("view.actualSize", tr("Actual Size"), {QKeySequence(Qt::CTRL | Qt::Key_1)},
        [this] { m_canvas->actualSize(); });
    add("view.fitWidth", tr("Fit Width"), {QKeySequence(Qt::CTRL | Qt::Key_2)},
        [this] { m_canvas->fitWidth(); });
    add("palette.show", tr("Command Palette…"), {QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_P)},
        [this] { m_palette->open(); });
}

MainWindow::~MainWindow() {
    // Qt would delete the canvas after the members, i.e. after the session it draws from.
    delete m_canvas;
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

void MainWindow::setDocumentStatus(const QString& text) {
    m_documentStatus->setText(text);
}

void MainWindow::chooseDocument() {
    const QString path =
        QFileDialog::getOpenFileName(this, tr("Open PDF"), QString(), tr("PDF documents (*.pdf)"));
    if (!path.isEmpty()) {
        openDocument(path);
    }
}

bool MainWindow::openDocument(const QString& path) {
    m_canvas->reset();
    m_pageStatus->clear();
    m_fileChanged = false;
    m_wrongPasswords = 0;
    ++m_openGeneration;
    m_repairBar->setReasons({});
    m_documentStatus->setToolTip(QString());
    const QString error = m_session.open(path);
    if (!error.isEmpty()) {
        setDocumentStatus(tr("Cannot open: %1").arg(error));
        return false;
    }
    m_fileName = QFileInfo(path).fileName();
    setWindowTitle(tr("%1 — Vellora").arg(m_fileName));
    setDocumentStatus(tr("Opening…"));
    return true;
}

void MainWindow::onOpened(quint32 pageCount, const QStringList& repairs) {
    // Also the answer of a restarted engine: it has the document again, so tiles are on their way.
    m_wrongPasswords = 0;
    m_canvas->hideBanner();
    statusBar()->clearMessage();
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

void MainWindow::onRequestFailed(quint64 request, const QString& message) {
    // A failure that belongs to a request is the canvas's business; one that belongs to no
    // request is the document (for example, the engine could not read the file).
    if (request == 0) {
        setDocumentStatus(tr("Cannot open: %1").arg(message));
    }
}

void MainWindow::onPasswordRequested(bool wrong) {
    setDocumentStatus(tr("Password required"));
    // The prompt is modal and runs its own event loop, which must not happen inside the session's
    // event dispatch: it is shown from the event loop instead, for the document that asked.
    const quint64 generation = m_openGeneration;
    QMetaObject::invokeMethod(
        this, [this, wrong, generation] { askForPassword(wrong, generation); },
        Qt::QueuedConnection);
}

void MainWindow::askForPassword(bool wrong, quint64 generation) {
    if (generation != m_openGeneration || !m_session.isOpen()) {
        return;
    }
    if (wrong) {
        ++m_wrongPasswords;
    }
    if (m_wrongPasswords >= kMaxPasswordAttempts) {
        giveUpOnDocument(
            tr("Cannot open: the password was wrong %n time(s)", nullptr, m_wrongPasswords));
        return;
    }
    const std::optional<QString> password = m_passwordProvider(
        m_fileName, m_wrongPasswords + 1, kMaxPasswordAttempts, m_wrongPasswords > 0);
    // The prompt ran its own event loop: the document may have been replaced meanwhile.
    if (generation != m_openGeneration || !m_session.isOpen()) {
        return;
    }
    if (!password) {
        giveUpOnDocument(tr("Cannot open: a password is required"));
        return;
    }
    const QString error = m_session.submitPassword(*password);
    if (!error.isEmpty()) {
        giveUpOnDocument(tr("Cannot open: %1").arg(error));
        return;
    }
    setDocumentStatus(tr("Opening…"));
}

void MainWindow::giveUpOnDocument(const QString& status) {
    m_session.close();
    m_fileName.clear();
    setWindowTitle(tr("Vellora"));
    setDocumentStatus(status);
}

void MainWindow::onEngineCrashed(const QString& how, bool willRestart) {
    // Non-modal: the window stays usable, and the banner goes away when the engine is back.
    m_canvas->showBanner(willRestart ? tr("Page failed to render — retrying…")
                                     : tr("The page renderer stopped and could not be restarted."));
    statusBar()->showMessage(willRestart ? tr("The engine stopped (%1) — restarting").arg(how)
                                         : tr("The engine stopped (%1)").arg(how));
}

void MainWindow::onFailed(const QString& reason) {
    m_canvas->showBanner(tr("The page renderer failed. Reopen the document."));
    setDocumentStatus(tr("Engine failed: %1").arg(reason));
}

void MainWindow::onEngineTimedOut(TimeoutStage stage, const QString& message) {
    if (stage == TimeoutStage::Tile) {
        // The engine was killed; `engineCrashed` follows and shows the restart.
        statusBar()->showMessage(tr("The page renderer stopped answering (%1)").arg(message));
        return;
    }
    m_canvas->showBanner(tr("The page renderer did not start in time. Reopen the document."));
    setDocumentStatus(tr("Engine timed out: %1").arg(message));
}

void MainWindow::onDocumentChanged(bool replaced) {
    // Reported just before the engine restarts over the file, which then shows what is there now.
    m_fileChanged = true;
    m_documentStatus->setToolTip(
        replaced ? tr("Another program replaced this file after it was opened. The pages shown "
                      "come from the file as it is now.")
                 : tr("Another program modified this file after it was opened. The pages shown "
                      "come from the file as it is now."));
}

void MainWindow::onPageChanged(quint32 page, quint32 pageCount) {
    m_pageStatus->setText(pageCount == 0 ? QString()
                                         : tr("Page %1 / %2").arg(page + 1).arg(pageCount));
}

void MainWindow::onZoomChanged(double zoom) {
    m_zoomStatus->setText(tr("%1%").arg(qRound(zoom * 100.0)));
}

} // namespace vellora
