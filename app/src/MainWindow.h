// The main window: native menu bar (File, View), the scrolling canvas, and a status bar with the
// page, the zoom and the document's state. A bar above the canvas says when the file was repaired.
// Task 22d routes every action through the command registry.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasView.h"
#include "commands/CommandPalette.h"
#include "commands/CommandRegistry.h"

#include <QMainWindow>
#include <QStringList>
#include <functional>
#include <optional>
#include <utility>

class QLabel;

namespace vellora {

class RepairBar;

class MainWindow : public QMainWindow {
    Q_OBJECT

public:
    explicit MainWindow(QWidget* parent = nullptr);
    ~MainWindow() override;

    // Opens `path` (the File -> Open dialog calls this with the chosen file). Shows the error in
    // the status bar and returns false if the engine cannot be started.
    bool openDocument(const QString& path);

    // How many wrong passwords end the attempt to open an encrypted document.
    static constexpr int kMaxPasswordAttempts = 3;
    // What the window asks when a document needs a password: the file name, the attempt (from 1),
    // the number of attempts and whether the previous password was refused. An empty optional is
    // "cancel". The default shows a modal `PasswordDialog`; tests replace it.
    using PasswordProvider = std::function<std::optional<QString>(
        const QString& fileName, int attempt, int maxAttempts, bool wrong)>;
    void setPasswordProvider(PasswordProvider provider) {
        m_passwordProvider = std::move(provider);
    }

    EngineSession& session() { return m_session; }
    CommandRegistry& commands() { return m_commands; }
    CommandPalette& palette() { return *m_palette; }
    CanvasView& canvas() { return *m_canvas; }
    RepairBar& repairBar() { return *m_repairBar; }
    // What the status bar shows, for tests.
    QString documentStatus() const;
    QString pageStatus() const;
    QString zoomStatus() const;

private slots:
    void chooseDocument();
    void onOpened(quint32 pageCount, const QStringList& repairs);
    void onRequestFailed(quint64 request, const QString& message);
    void onPasswordRequested(bool wrong);
    void onEngineCrashed(const QString& how, bool willRestart);
    void onFailed(const QString& reason);
    void onEngineTimedOut(TimeoutStage stage, const QString& message);
    void onDocumentChanged(bool replaced);
    void onPageChanged(quint32 page, quint32 pageCount);
    void onZoomChanged(double zoom);

private:
    void registerCommands();
    void setDocumentStatus(const QString& text);
    void askForPassword(bool wrong, quint64 generation);
    void giveUpOnDocument(const QString& status);

    // The canvas uses the session, so the destructor deletes the canvas before this member goes.
    EngineSession m_session;
    CommandRegistry m_commands;
    CanvasView* m_canvas = nullptr;
    RepairBar* m_repairBar = nullptr;
    CommandPalette* m_palette = nullptr;
    QLabel* m_documentStatus = nullptr;
    QLabel* m_pageStatus = nullptr;
    QLabel* m_zoomStatus = nullptr;
    QString m_fileName;
    // The file changed on disk since it was opened; shown next to the page count.
    bool m_fileChanged = false;
    PasswordProvider m_passwordProvider;
    // Wrong passwords for the document being opened; never more than `kMaxPasswordAttempts`.
    int m_wrongPasswords = 0;
    // Counts `openDocument` calls, so that a prompt queued for one document is not shown for the
    // next.
    quint64 m_openGeneration = 0;
};

} // namespace vellora
