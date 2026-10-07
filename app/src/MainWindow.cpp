#include "MainWindow.h"

#include <QAction>
#include <QFileDialog>
#include <QFileInfo>
#include <QLabel>
#include <QMenuBar>
#include <QStatusBar>

namespace vellora {

MainWindow::MainWindow(QWidget* parent) : QMainWindow(parent) {
    setWindowTitle(tr("Vellora"));
    resize(1000, 800);

    m_placeholder = new QLabel(tr("Open a PDF with File → Open"), this);
    m_placeholder->setAlignment(Qt::AlignCenter);
    setCentralWidget(m_placeholder);

    m_documentStatus = new QLabel(this);
    statusBar()->addPermanentWidget(m_documentStatus);

    auto* fileMenu = menuBar()->addMenu(tr("&File"));
    auto* openAction = fileMenu->addAction(tr("&Open…"));
    openAction->setShortcut(QKeySequence::Open);
    connect(openAction, &QAction::triggered, this, &MainWindow::chooseDocument);
    fileMenu->addSeparator();
    auto* quitAction = fileMenu->addAction(tr("&Quit"));
    quitAction->setShortcut(QKeySequence::Quit);
    connect(quitAction, &QAction::triggered, this, &QWidget::close);

    connect(&m_session, &EngineSession::opened, this, &MainWindow::onOpened);
    connect(&m_session, &EngineSession::requestFailed, this, &MainWindow::onRequestFailed);
    connect(&m_session, &EngineSession::engineCrashed, this,
            [this](const QString& how, bool willRestart, const QList<quint64>&) {
                onEngineCrashed(how, willRestart);
            });
    connect(&m_session, &EngineSession::failed, this, &MainWindow::onFailed);
}

QString MainWindow::documentStatus() const {
    return m_documentStatus->text();
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

void MainWindow::onOpened(quint32 pageCount, bool repaired) {
    QString text = tr("%n page(s)", nullptr, static_cast<int>(pageCount));
    if (repaired) {
        text += tr(" — repaired");
    }
    setDocumentStatus(text);
    m_placeholder->setText(QStringLiteral("%1\n%2").arg(m_fileName, text));
}

void MainWindow::onRequestFailed(quint64 request, const QString& message) {
    // A failure that belongs to a request is the canvas's business (22c); one that belongs to no
    // request is the document (for example, the engine could not read the file).
    if (request == 0) {
        setDocumentStatus(tr("Cannot open: %1").arg(message));
    }
}

void MainWindow::onEngineCrashed(const QString& how, bool willRestart) {
    statusBar()->showMessage(willRestart ? tr("The engine stopped (%1) — restarting").arg(how)
                                         : tr("The engine stopped (%1)").arg(how));
}

void MainWindow::onFailed(const QString& reason) {
    setDocumentStatus(tr("Engine failed: %1").arg(reason));
}

} // namespace vellora
