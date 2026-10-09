// One open document: its engine session, its canvas, the repair bar and the texts the status bar
// shows for it. A tab owns its engine process; destroying the tab ends the process.
//
// The tab also asks for the password of an encrypted document (three attempts, never logged or
// stored) and puts the view back where the user left it when the same file is opened again.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasView.h"
#include "settings/AppSettings.h"

#include <QWidget>
#include <functional>
#include <optional>
#include <utility>

class QSplitter;

namespace vellora {

class RepairBar;
class ThumbnailSidebar;

class DocumentTab : public QWidget {
    Q_OBJECT

public:
    // How many wrong passwords end the attempt to open an encrypted document.
    static constexpr int kMaxPasswordAttempts = 3;
    // What the tab asks when the document needs a password: the file name, the attempt (from 1),
    // the number of attempts and whether the previous password was refused. An empty optional is
    // "cancel". The default shows a modal `PasswordDialog`; tests replace it.
    using PasswordProvider = std::function<std::optional<QString>(
        const QString& fileName, int attempt, int maxAttempts, bool wrong)>;

    // `settings` is not owned and may be null (nothing is remembered then).
    explicit DocumentTab(AppSettings* settings, QWidget* parent = nullptr);
    ~DocumentTab() override;

    // Opens `path` in this tab, replacing what it showed. False (with the reason in
    // `documentStatus`) if the engine cannot be started.
    bool open(const QString& path);

    // Empty until a document has been given to `open`.
    QString path() const { return m_path; }
    QString fileName() const { return m_fileName; }
    bool isEmpty() const { return m_path.isEmpty(); }

    EngineSession& session() { return m_session; }
    CanvasView& canvas() { return *m_canvas; }
    RepairBar& repairBar() { return *m_repairBar; }
    ThumbnailSidebar& sidebar() { return *m_sidebar; }

    // The thumbnails on the left. Hidden until asked for (or until the user's last choice, which
    // the settings keep); its width and its thumbnails' size are remembered the same way.
    bool sidebarVisible() const;
    void setSidebarVisible(bool visible);

    QString documentStatus() const { return m_documentStatus; }
    QString documentStatusTip() const { return m_documentTip; }
    QString pageStatus() const { return m_pageStatus; }
    QString zoomStatus() const { return m_zoomStatus; }

    void setPasswordProvider(PasswordProvider provider) {
        m_passwordProvider = std::move(provider);
    }

    // Remembers where the document is left, for the next time the same file is opened. Does
    // nothing for a tab that has no open document.
    void saveViewState();

signals:
    // A status text changed (document, page, zoom).
    void statusChanged();
    // The file name (and so the tab title) changed.
    void titleChanged();
    // The arrangement of the pages or the turn of the view changed.
    void viewModeChanged();
    // The sidebar was shown or hidden.
    void sidebarVisibilityChanged(bool visible);
    // A short message for the status bar; empty clears it.
    void message(const QString& text);
    // The tab wants the user's attention (a password prompt): show it.
    void needsAttention();

private slots:
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
    AppSettings::Layout currentLayout() const;
    void applyLayout(const AppSettings::Layout& layout);
    void setDocumentStatus(const QString& text);
    void askForPassword(bool wrong, quint64 generation);
    void giveUp(const QString& status);

    void saveSidebar();

    AppSettings* m_settings;
    // The canvas and the sidebar use the session, so the destructor deletes them before this member
    // goes.
    EngineSession m_session;
    QSplitter* m_splitter = nullptr;
    ThumbnailSidebar* m_sidebar = nullptr;
    CanvasView* m_canvas = nullptr;
    RepairBar* m_repairBar = nullptr;
    QString m_path;
    QString m_fileName;
    QString m_documentStatus;
    QString m_documentTip;
    QString m_pageStatus;
    QString m_zoomStatus;
    // The file changed on disk since it was opened; shown next to the page count.
    bool m_fileChanged = false;
    PasswordProvider m_passwordProvider;
    // Wrong passwords for the document being opened; never more than `kMaxPasswordAttempts`.
    int m_wrongPasswords = 0;
    // Counts `open` calls, so that a prompt queued for one document is not shown for the next.
    quint64 m_generation = 0;
    // Where the user left this version of the file, applied once when it first opens.
    QString m_viewKey;
    std::optional<AppSettings::ViewState> m_savedView;
    bool m_opened = false;
    bool m_applyingLayout = false;
};

} // namespace vellora