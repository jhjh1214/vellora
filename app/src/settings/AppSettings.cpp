#include "settings/AppSettings.h"

#include <QCryptographicHash>
#include <QDateTime>
#include <QDir>
#include <algorithm>
#include <cmath>

namespace vellora {

namespace {

constexpr auto kGeometry = "window/geometry";
constexpr auto kLastDirectory = "files/lastDirectory";
constexpr auto kCrashSeen = "crashes/seenUntilMs";
constexpr auto kLayout = "view/layout";
constexpr auto kRecent = "files/recent";
constexpr auto kViewStates = "views";

#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
constexpr Qt::CaseSensitivity kPathCase = Qt::CaseInsensitive;
#else
constexpr Qt::CaseSensitivity kPathCase = Qt::CaseSensitive;
#endif

struct Entry {
    QString key;
    AppSettings::ViewState state;
};

QList<Entry> readEntries(QSettings& settings) {
    QList<Entry> entries;
    const int count = settings.beginReadArray(QLatin1String(kViewStates));
    entries.reserve(std::min(count, AppSettings::kMaxViewStates));
    for (int i = 0; i < count && entries.size() < AppSettings::kMaxViewStates; ++i) {
        settings.setArrayIndex(i);
        Entry entry;
        entry.key = settings.value(QStringLiteral("key")).toString();
        bool pageOk = false;
        bool offsetOk = false;
        bool zoomOk = false;
        const uint page = settings.value(QStringLiteral("page")).toUInt(&pageOk);
        entry.state.page = page;
        entry.state.offsetPoints = settings.value(QStringLiteral("offset")).toDouble(&offsetOk);
        entry.state.zoom = settings.value(QStringLiteral("zoom")).toDouble(&zoomOk);
        // Entries from before view modes have no arrangement: the usual one.
        entry.state.layout.continuous = settings.value(QStringLiteral("continuous"), true).toBool();
        entry.state.layout.spread =
            std::clamp(settings.value(QStringLiteral("spread"), 0).toInt(), 0, 2);
        entry.state.layout.rotation =
            ((settings.value(QStringLiteral("rotation"), 0).toInt() % 4) + 4) % 4;
        // The file is the user's, but it is read back as untrusted: ignore what is not a number.
        if (!entry.key.isEmpty() && pageOk && offsetOk && zoomOk &&
            std::isfinite(entry.state.offsetPoints) && std::isfinite(entry.state.zoom)) {
            entries.append(entry);
        }
    }
    settings.endArray();
    return entries;
}

} // namespace

QString normalizedPath(const QString& path) {
    return QDir::cleanPath(QFileInfo(path).absoluteFilePath());
}

QStringList resolvePaths(const QStringList& files, const QString& baseDir) {
    QStringList resolved;
    const QDir base(baseDir);
    for (const QString& file : files) {
        if (!file.isEmpty()) {
            resolved.append(QDir::cleanPath(base.absoluteFilePath(file)));
        }
    }
    return resolved;
}

bool samePath(const QString& a, const QString& b) {
    return normalizedPath(a).compare(normalizedPath(b), kPathCase) == 0;
}

QByteArray AppSettings::windowGeometry() const {
    return m_settings ? m_settings->value(QLatin1String(kGeometry)).toByteArray() : QByteArray();
}

void AppSettings::setWindowGeometry(const QByteArray& geometry) {
    if (m_settings) {
        m_settings->setValue(QLatin1String(kGeometry), geometry);
    }
}

QString AppSettings::lastDirectory() const {
    return m_settings ? m_settings->value(QLatin1String(kLastDirectory)).toString() : QString();
}

void AppSettings::setLastDirectory(const QString& directory) {
    if (m_settings) {
        m_settings->setValue(QLatin1String(kLastDirectory), directory);
    }
}

AppSettings::Layout AppSettings::lastLayout() const {
    Layout layout;
    if (m_settings) {
        const QStringList parts = m_settings->value(QLatin1String(kLayout)).toStringList();
        if (parts.size() == 3) {
            layout.continuous = parts.at(0) == QLatin1String("1");
            layout.spread = std::clamp(parts.at(1).toInt(), 0, 2);
            layout.rotation = ((parts.at(2).toInt() % 4) + 4) % 4;
        }
    }
    return layout;
}

void AppSettings::setLastLayout(const Layout& layout) {
    if (m_settings) {
        m_settings->setValue(
            QLatin1String(kLayout),
            QStringList{layout.continuous ? QStringLiteral("1") : QStringLiteral("0"),
                        QString::number(layout.spread), QString::number(layout.rotation)});
    }
}

QDateTime AppSettings::crashReportsSeenUntil() const {
    if (!m_settings) {
        return {};
    }
    bool ok = false;
    const qint64 ms = m_settings->value(QLatin1String(kCrashSeen)).toLongLong(&ok);
    return ok && ms > 0 ? QDateTime::fromMSecsSinceEpoch(ms) : QDateTime();
}

void AppSettings::setCrashReportsSeenUntil(const QDateTime& when) {
    if (m_settings && when.isValid()) {
        m_settings->setValue(QLatin1String(kCrashSeen), when.toMSecsSinceEpoch());
    }
}

QStringList AppSettings::recentFiles() const {
    if (!m_settings) {
        return {};
    }
    QStringList files = m_settings->value(QLatin1String(kRecent)).toStringList();
    files.removeAll(QString());
    while (files.size() > kMaxRecentFiles) {
        files.removeLast();
    }
    return files;
}

void AppSettings::addRecentFile(const QString& path) {
    if (!m_settings || path.isEmpty()) {
        return;
    }
    removeRecentFile(path);
    QStringList files = recentFiles();
    files.prepend(normalizedPath(path));
    while (files.size() > kMaxRecentFiles) {
        files.removeLast();
    }
    m_settings->setValue(QLatin1String(kRecent), files);
}

void AppSettings::removeRecentFile(const QString& path) {
    if (!m_settings) {
        return;
    }
    QStringList files = recentFiles();
    files.erase(std::remove_if(files.begin(), files.end(),
                               [&](const QString& file) { return samePath(file, path); }),
                files.end());
    m_settings->setValue(QLatin1String(kRecent), files);
}

void AppSettings::clearRecentFiles() {
    if (m_settings) {
        m_settings->setValue(QLatin1String(kRecent), QStringList());
    }
}

QString AppSettings::viewKey(const QFileInfo& file) {
    const QString identity = QStringLiteral("%1|%2|%3")
                                 .arg(normalizedPath(file.filePath()), QString::number(file.size()),
                                      QString::number(file.lastModified().toMSecsSinceEpoch()));
    return QString::fromLatin1(
        QCryptographicHash::hash(identity.toUtf8(), QCryptographicHash::Sha256).toHex());
}

std::optional<AppSettings::ViewState> AppSettings::viewState(const QString& key) const {
    if (!m_settings || key.isEmpty()) {
        return std::nullopt;
    }
    for (const Entry& entry : readEntries(*m_settings)) {
        if (entry.key == key) {
            return entry.state;
        }
    }
    return std::nullopt;
}

void AppSettings::setViewState(const QString& key, const ViewState& state) {
    if (!m_settings || key.isEmpty() || !std::isfinite(state.zoom) ||
        !std::isfinite(state.offsetPoints)) {
        return;
    }
    QList<Entry> entries = readEntries(*m_settings);
    entries.erase(std::remove_if(entries.begin(), entries.end(),
                                 [&](const Entry& entry) { return entry.key == key; }),
                  entries.end());
    entries.append({key, state});
    while (entries.size() > kMaxViewStates) {
        entries.removeFirst();
    }
    m_settings->remove(QLatin1String(kViewStates));
    m_settings->beginWriteArray(QLatin1String(kViewStates), static_cast<int>(entries.size()));
    for (int i = 0; i < entries.size(); ++i) {
        m_settings->setArrayIndex(i);
        m_settings->setValue(QStringLiteral("key"), entries.at(i).key);
        m_settings->setValue(QStringLiteral("page"), entries.at(i).state.page);
        m_settings->setValue(QStringLiteral("offset"), entries.at(i).state.offsetPoints);
        m_settings->setValue(QStringLiteral("zoom"), entries.at(i).state.zoom);
        m_settings->setValue(QStringLiteral("continuous"), entries.at(i).state.layout.continuous);
        m_settings->setValue(QStringLiteral("spread"), entries.at(i).state.layout.spread);
        m_settings->setValue(QStringLiteral("rotation"), entries.at(i).state.layout.rotation);
    }
    m_settings->endArray();
}

int AppSettings::viewStateCount() const {
    return m_settings ? static_cast<int>(readEntries(*m_settings).size()) : 0;
}

void AppSettings::sync() {
    if (m_settings) {
        m_settings->sync();
    }
}

} // namespace vellora