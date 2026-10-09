// The application log. Starting it makes the Rust side write rotating files (the engine's log lines
// included, see `vellora-engine-client`'s `logging` module) and routes `qInfo`, `qWarning`,
// `qCritical` and `qDebug` into the same files. Messages still go on to the handler that was
// installed before (the console, or Qt Test's), so debugging works as usual.
//
// Never log document content at info or above, and never a password: the messages end up in files
// the user is asked to attach to bug reports.
#pragma once

#include <QString>

namespace vellora {

class Logging {
public:
    // The platform's log folder (`VELLORA_LOG_DIR` overrides it).
    static QString defaultDirectory();

    // Starts logging into `directory` (the default folder if empty). Returns an empty string on
    // success, else why not (already started, folder not writable).
    static QString start(const QString& directory = {});
    // Writes what is queued, stops logging and puts the earlier message handler back.
    static void stop();
    // Waits until everything logged so far is in the files.
    static void flush();
    static bool isStarted();
    // The folder in use while started, else the default one.
    static QString directory();
};

} // namespace vellora