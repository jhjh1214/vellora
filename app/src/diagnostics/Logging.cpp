#include "diagnostics/Logging.h"

#include "bridge/EngineSession.h"

#include <QMutex>
#include <QMutexLocker>
#include <exception>
#include <optional>

namespace vellora {

namespace {

QMutex g_mutex;
std::optional<rust::Box<LogHandle>> g_log;
QString g_directory;
QtMessageHandler g_previousHandler = nullptr;
// A message produced while logging a message (by the logging code itself) is not logged again.
thread_local bool g_inHandler = false;

QString toQString(const rust::String& text) {
    return QString::fromUtf8(text.data(), static_cast<qsizetype>(text.size()));
}

void forward(QtMsgType type, const QMessageLogContext& context, const QString& message) {
    if (!g_inHandler) {
        g_inHandler = true;
        const QByteArray utf8 = message.toUtf8();
        LogLevel level = LogLevel::Debug;
        switch (type) {
        case QtDebugMsg:
            level = LogLevel::Debug;
            break;
        case QtInfoMsg:
            level = LogLevel::Info;
            break;
        case QtWarningMsg:
            level = LogLevel::Warning;
            break;
        case QtCriticalMsg:
        case QtFatalMsg:
            level = LogLevel::Error;
            break;
        }
        // Not under the lock: logging is thread safe, and `stop` only drops the handle after it
        // has put the old message handler back.
        try {
            vellora::log_message(level,
                                 rust::Str(utf8.constData(), static_cast<size_t>(utf8.size())));
        } catch (const std::exception&) {
            // Logging must never be the reason for a failure.
        }
        g_inHandler = false;
    }
    if (g_previousHandler != nullptr) {
        g_previousHandler(type, context, message);
    }
}

} // namespace

QString Logging::defaultDirectory() {
    return toQString(vellora::default_log_directory());
}

QString Logging::start(const QString& directory) {
    QMutexLocker lock(&g_mutex);
    if (g_log) {
        return QObject::tr("Logging is already started.");
    }
    try {
        const QByteArray utf8 = directory.toUtf8();
        g_log.emplace(
            vellora::start_logging(rust::Str(utf8.constData(), static_cast<size_t>(utf8.size()))));
    } catch (const std::exception& error) {
        return QString::fromUtf8(error.what());
    }
    g_directory = directory.isEmpty() ? defaultDirectory() : directory;
    g_previousHandler = qInstallMessageHandler(forward);
    return {};
}

void Logging::stop() {
    QMutexLocker lock(&g_mutex);
    if (!g_log) {
        return;
    }
    qInstallMessageHandler(g_previousHandler);
    g_previousHandler = nullptr;
    (*g_log)->stop();
    g_log.reset();
}

void Logging::flush() {
    QMutexLocker lock(&g_mutex);
    if (g_log) {
        (*g_log)->flush();
    }
}

bool Logging::isStarted() {
    QMutexLocker lock(&g_mutex);
    return g_log.has_value();
}

QString Logging::directory() {
    QMutexLocker lock(&g_mutex);
    return g_log ? g_directory : defaultDirectory();
}

} // namespace vellora