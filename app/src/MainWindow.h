// The main window: native menu bar (File, View), the scrolling canvas, and a status bar with the
// page, the zoom and the document's state. Task 22d routes every action through the command
// registry.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasView.h"
#include "commands/CommandPalette.h"
#include "commands/CommandRegistry.h"

#include <QMainWindow>

class QLabel;

namespace vellora {

class MainWindow : public QMainWindow {
    Q_OBJECT

public:
    explicit MainWindow(QWidget* parent = nullptr);
    ~MainWindow() override;

    // Opens `path` (the File -> Open dialog calls this with the chosen file). Shows the error in
    // the status bar and returns false if the engine cannot be started.
    bool openDocument(const QString& path);

    EngineSession& session() { return m_session; }
    CommandRegistry& commands() { return m_commands; }
    CommandPalette& palette() { return *m_palette; }
    CanvasView& canvas() { return *m_canvas; }
    // What the status bar shows, for tests.
    QString documentStatus() const;
    QString pageStatus() const;
    QString zoomStatus() const;

private slots:
    void chooseDocument();
    void onOpened(quint32 pageCount, bool repaired);
    void onRequestFailed(quint64 request, const QString& message);
    void onEngineCrashed(const QString& how, bool willRestart);
    void onFailed(const QString& reason);
    void onPageChanged(quint32 page, quint32 pageCount);
    void onZoomChanged(double zoom);

private:
    void registerCommands();
    void setDocumentStatus(const QString& text);

    // The canvas uses the session, so the destructor deletes the canvas before this member goes.
    EngineSession m_session;
    CommandRegistry m_commands;
    CanvasView* m_canvas = nullptr;
    CommandPalette* m_palette = nullptr;
    QLabel* m_documentStatus = nullptr;
    QLabel* m_pageStatus = nullptr;
    QLabel* m_zoomStatus = nullptr;
    QString m_fileName;
};

} // namespace vellora
