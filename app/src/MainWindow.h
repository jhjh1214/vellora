// The main window: native menu bar, File -> Open, and a status bar. Later tasks add the scrolling
// canvas (22c) and route every action through the command registry (22d).
#pragma once

#include "bridge/EngineSession.h"

#include <QMainWindow>

class QLabel;

namespace vellora {

class MainWindow : public QMainWindow {
    Q_OBJECT

public:
    explicit MainWindow(QWidget* parent = nullptr);

    // Opens `path` (the File -> Open dialog calls this with the chosen file). Shows the error in
    // the status bar and returns false if the engine cannot be started.
    bool openDocument(const QString& path);

    EngineSession& session() { return m_session; }
    // What the status bar shows for the document, for tests.
    QString documentStatus() const;

private slots:
    void chooseDocument();
    void onOpened(quint32 pageCount, bool repaired);
    void onRequestFailed(quint64 request, const QString& message);
    void onEngineCrashed(const QString& how, bool willRestart);
    void onFailed(const QString& reason);

private:
    void setDocumentStatus(const QString& text);

    EngineSession m_session;
    QLabel* m_placeholder = nullptr;
    QLabel* m_documentStatus = nullptr;
    QString m_fileName;
};

} // namespace vellora
