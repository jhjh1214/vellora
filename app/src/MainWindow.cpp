#include "MainWindow.h"

#include <QAction>
#include <QFileDialog>
#include <QFileInfo>
#include <QLabel>
#include <QMenuBar>
#include <QScreen>
#include <QStatusBar>
#include <QStyle>

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
    // 1000 x 800, but never more than 80% of the screen.
    const QSize available = screen() ? screen()->availableSize() : QSize(1250, 1000);
    resize(QSize(1000, 800).boundedTo(available * 0.8));
    if (screen()) {
        // Centred, so that the status bar is not below the bottom of a small screen.
        setGeometry(QStyle::alignedRect(Qt::LeftToRight, Qt::AlignCenter, size(),
                                        screen()->availableGeometry()));
    }

    m_canvas = new CanvasView(&m_session, this);
    setCentralWidget(m_canvas);

    m_documentStatus = plainLabel(this);
    m_pageStatus = plainLabel(this);
    m_zoomStatus = plainLabel(this);
    statusBar()->addPermanentWidget(m_documentStatus);
    statusBar()->addPermanentWidget(m_pageStatus);
    statusBar()->addPermanentWidget(m_zoomStatus);
    onZoomChanged(m_canvas->controller()->zoom());

    auto* fileMenu = menuBar()->addMenu(tr("&File"));
    auto* openAction = fileMenu->addAction(tr("&Open…"));
    openAction->setShortcut(QKeySequence::Open);
    connect(openAction, &QAction::triggered, this, &MainWindow::chooseDocument);
    fileMenu->addSeparator();
    auto* quitAction = fileMenu->addAction(tr("&Quit"));
    quitAction->setShortcut(QKeySequence::Quit);
    connect(quitAction, &QAction::triggered, this, &QWidget::close);

    auto* viewMenu = menuBar()->addMenu(tr("&View"));
    auto* zoomIn = viewMenu->addAction(tr("Zoom &In"));
    zoomIn->setShortcuts({QKeySequence::ZoomIn, QKeySequence(Qt::CTRL | Qt::Key_Equal)});
    connect(zoomIn, &QAction::triggered, m_canvas, &CanvasView::zoomIn);
    auto* zoomOut = viewMenu->addAction(tr("Zoom &Out"));
    zoomOut->setShortcut(QKeySequence::ZoomOut);
    connect(zoomOut, &QAction::triggered, m_canvas, &CanvasView::zoomOut);
    auto* actualSize = viewMenu->addAction(tr("&Actual Size"));
    actualSize->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_1));
    connect(actualSize, &QAction::triggered, m_canvas, &CanvasView::actualSize);
    auto* fitWidth = viewMenu->addAction(tr("Fit &Width"));
    fitWidth->setShortcut(QKeySequence(Qt::CTRL | Qt::Key_2));
    connect(fitWidth, &QAction::triggered, m_canvas, &CanvasView::fitWidth);

    connect(&m_session, &EngineSession::opened, this, &MainWindow::onOpened);
    connect(&m_session, &EngineSession::requestFailed, this, &MainWindow::onRequestFailed);
    connect(&m_session, &EngineSession::engineCrashed, this,
            [this](const QString& how, bool willRestart, const QList<quint64>&) {
                onEngineCrashed(how, willRestart);
            });
    connect(&m_session, &EngineSession::failed, this, &MainWindow::onFailed);
    connect(m_canvas->controller(), &CanvasController::currentPageChanged, this,
            &MainWindow::onPageChanged);
    connect(m_canvas->controller(), &CanvasController::zoomChanged, this,
            &MainWindow::onZoomChanged);
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
}

void MainWindow::onRequestFailed(quint64 request, const QString& message) {
    // A failure that belongs to a request is the canvas's business; one that belongs to no
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

void MainWindow::onPageChanged(quint32 page, quint32 pageCount) {
    m_pageStatus->setText(pageCount == 0 ? QString()
                                         : tr("Page %1 / %2").arg(page + 1).arg(pageCount));
}

void MainWindow::onZoomChanged(double zoom) {
    m_zoomStatus->setText(tr("%1%").arg(qRound(zoom * 100.0)));
}

} // namespace vellora
