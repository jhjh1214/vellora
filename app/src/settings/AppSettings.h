// What the application remembers between runs, over a QSettings (native format: the registry on
// Windows, a plist on macOS, an ini file on Linux). A null QSettings means "remember nothing",
// which is what tests and the window's default use, so that nothing but the real application ever
// touches the user's settings.
//
// Nothing here is document content: paths, window geometry and the place where a document was left.
// "Clear recent files" removes the paths; the view states hold no path (see `viewKey`).
#pragma once

#include <QByteArray>
#include <QDateTime>
#include <QFileInfo>
#include <QSettings>
#include <QString>
#include <QStringList>
#include <optional>

namespace vellora {

class AppSettings {
public:
    static constexpr int kMaxRecentFiles = 15;
    static constexpr int kMaxViewStates = 500;

    // Where a document was left: the page at the top of the window, how far into it (in points)
    // and the zoom. The view mode joins them when there are view modes (M1 task 10).
    struct ViewState {
        quint32 page = 0;
        double offsetPoints = 0.0;
        double zoom = 1.0;
    };

    // Not owned; may be null.
    explicit AppSettings(QSettings* settings = nullptr) : m_settings(settings) {}

    QByteArray windowGeometry() const;
    void setWindowGeometry(const QByteArray& geometry);

    QString lastDirectory() const;
    void setLastDirectory(const QString& directory);

    // Crash reports modified before this have been shown to the user (null: none yet).
    QDateTime crashReportsSeenUntil() const;
    void setCrashReportsSeenUntil(const QDateTime& when);

    // Most recent first, at most `kMaxRecentFiles`, absolute and cleaned paths.
    QStringList recentFiles() const;
    void addRecentFile(const QString& path);
    void removeRecentFile(const QString& path);
    void clearRecentFiles();

    // Identifies one version of one file: path, size and modification time. A file that was
    // changed (or replaced) on disk has another key, so an old position is never applied to it.
    // The key is a hash and does not reveal the path.
    static QString viewKey(const QFileInfo& file);
    std::optional<ViewState> viewState(const QString& key) const;
    // Remembers the state as the most recent of at most `kMaxViewStates`; the least recent goes.
    void setViewState(const QString& key, const ViewState& state);
    int viewStateCount() const;

    void sync();

private:
    QSettings* m_settings;
};

// The path as the application stores and compares it: absolute and cleaned.
QString normalizedPath(const QString& path);
// The paths as given on a command line, made absolute against `baseDir` (the directory the command
// was run in) and cleaned. Order is kept; empty arguments are dropped.
QStringList resolvePaths(const QStringList& files, const QString& baseDir);
// Whether two paths name the same file as far as their text goes (case-insensitive on Windows and
// macOS, whose file systems are, by default).
bool samePath(const QString& a, const QString& b);

} // namespace vellora