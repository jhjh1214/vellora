// One open document: its engine session, its canvas, the repair bar and the texts the status bar
// shows for it. A tab owns its engine process; destroying the tab ends the process.
//
// The tab also asks for the password of an encrypted document (three attempts, never logged or
// stored) and puts the view back where the user left it when the same file is opened again.
#pragma once

#include "bridge/EngineSession.h"
#include "canvas/CanvasView.h"
#include "navigation/NavigationHistory.h"
#include "settings/AppSettings.h"

#include <QSet>
#include <QUrl>
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
    // Page labels are read in windows of this many pages (the protocol's most), up to
    // `kMaxLabelledPages`; beyond that pages show their numbers.
    static constexpr quint32 kLabelsPerRequest = 1024;
    static constexpr quint32 kMaxLabelledPages = 1U << 16;
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
    NavigationHistory& history() { return m_history; }

    // ---- navigation ----
    // Jumps are recorded in the history (Back and Forward return to where the reader was);
    // scrolling, page turns and zooming are not.
    //
    // Follows a destination of the document (an outline item).
    void jumpTo(const Destination& destination);
    void jumpToPage(quint32 page);
    // Back and Forward in the history; false if there is nowhere to go.
    bool goBack();
    bool goForward();
    // The place at the top edge of the window now.
    NavigationHistory::Place currentPlace() const;
    // Goes to the page whose label is `text` ("iv", "A-3"), else to the page of that number. The
    // engine is asked, so the jump follows a moment later; if there is no such page, `message`
    // says so.
    void goToPageText(const QString& text);
    // ---- links ----
    // What clicking a link does. A jump inside the document is followed (and recorded in the
    // history); an address is opened only after the reader confirms it, for the schemes http,
    // https and mailto (anything else is shown and refused); every other kind of action is
    // described and never run.
    void activateLink(const Link& link);
    // The question before an address is opened: whether to open it, and whether to stop asking
    // for its host in this document. The default shows a `UriConfirmDialog`; tests replace it.
    struct UriChoice {
        bool open = false;
        bool trustHost = false;
    };
    using UriConfirmer = std::function<UriChoice(const QString& uri, const QString& host)>;
    void setUriConfirmer(UriConfirmer confirmer) { m_uriConfirmer = std::move(confirmer); }
    // What opens an address (the default asks the desktop); tests replace it.
    using UriOpener = std::function<bool(const QUrl& url)>;
    void setUriOpener(UriOpener opener) { m_uriOpener = std::move(opener); }
    // What tells the reader something that needs reading (a refused address, an action never
    // run). The default shows a message box; tests replace it.
    using Notifier = std::function<void(const QString& text)>;
    void setNotifier(Notifier notifier) { m_notifier = std::move(notifier); }
    // Hosts the reader chose not to be asked about again, for this document and this run.
    bool isHostTrusted(const QString& host) const { return m_trustedHosts.contains(host); }

    // Shows the sidebar on its outline tab.
    void showOutline();
    // The label of a page: the document's own if it has page labels, else its number.
    QString pageLabel(quint32 page) const;
    // Whether the document defines page labels (known once the engine has answered).
    bool hasPageLabels() const { return m_labelsDefined; }

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
    // Back or Forward became possible or impossible.
    void historyChanged();
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
    void onPageLabels(quint64 request, quint32 first, bool defined, const QStringList& labels);
    void onPageFound(quint64 request, bool found, quint32 page);
    void onZoomChanged(double zoom);

private:
    AppSettings::Layout currentLayout() const;
    void applyLayout(const AppSettings::Layout& layout);
    void setDocumentStatus(const QString& text);
    void askForPassword(bool wrong, quint64 generation);
    void giveUp(const QString& status);

    void saveSidebar();
    void requestLabels(quint32 first);
    void refreshPageStatus();
    // A page typed by number, the fallback when no page has that label.
    void goToPageNumber(const QString& text);
    void openUri(const QString& uri);
    void notify(const QString& text);

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
    UriConfirmer m_uriConfirmer;
    UriOpener m_uriOpener;
    Notifier m_notifier;
    QSet<QString> m_trustedHosts;
    NavigationHistory m_history;
    // The page labels, read in windows after the document opens (empty if it defines none).
    QStringList m_labels;
    bool m_labelsDefined = false;
    quint64 m_labelRequest = 0;
    // The page label being looked for, and the request that looks.
    QString m_findText;
    quint64 m_findRequest = 0;
    quint32 m_page = 0;
    quint32 m_pageCount = 0;
};

} // namespace vellora