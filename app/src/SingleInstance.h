// One running instance per user: a second `vellora file.pdf` hands its files to the first and
// exits.
//
// The first instance listens on a per-user local socket (a named pipe on Windows, a Unix socket
// elsewhere, reachable by the owner only). A later launch connects, sends the absolute paths it was
// given and leaves. What arrives is only a list of paths, capped in count and size; the receiver
// opens them like any other file, so a hostile local process gains nothing it could not do by
// starting `vellora` itself.
#pragma once

#include <QLocalServer>
#include <QObject>
#include <QStringList>

namespace vellora {

class SingleInstance : public QObject {
    Q_OBJECT

public:
    // Most paths and bytes one message may carry.
    static constexpr int kMaxFiles = 64;
    static constexpr qint64 kMaxMessageBytes = 1 << 20;

    // `name` identifies the instance group; the default is per user and per application.
    static QString defaultName();

    explicit SingleInstance(const QString& name = defaultName(), QObject* parent = nullptr);

    // Sends `files` to the instance that is running, if there is one. Blocks for at most
    // `timeoutMs`. True if a running instance took the message (and so this process should exit).
    // Call it before `listen`, from the thread that has no event loop yet, or from any thread.
    bool forward(const QStringList& files, int timeoutMs = 2000) const;

    // Becomes the running instance. False if the socket cannot be created. A socket file left
    // by a crashed instance is removed first; call `forward` before, so that a live instance is
    // never displaced.
    bool listen();
    bool isListening() const { return m_server.isListening(); }

signals:
    // Files another launch asked for (possibly none: it only wanted the window in front).
    void filesReceived(const QStringList& files);

private:
    void onNewConnection();

    QString m_name;
    QLocalServer m_server;
};

} // namespace vellora