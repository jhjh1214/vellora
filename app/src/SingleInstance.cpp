#include "SingleInstance.h"

#include <QCoreApplication>
#include <QCryptographicHash>
#include <QDataStream>
#include <QLocalSocket>
#include <QSharedPointer>
#include <QStandardPaths>
#include <QTimer>

namespace vellora {

namespace {

constexpr quint32 kMagic = 0x564C4C41; // "VLLA"
// A client that connects and says nothing must not hold the receiver.
constexpr int kReadTimeoutMs = 2000;

} // namespace

QString SingleInstance::defaultName() {
    // The user is part of the name where the platform's socket directory is shared (Windows pipes).
    const QString user = QStandardPaths::writableLocation(QStandardPaths::HomeLocation);
    const QByteArray hash =
        QCryptographicHash::hash(user.toUtf8(), QCryptographicHash::Sha256).toHex().left(16);
    return QStringLiteral("vellora-%1").arg(QString::fromLatin1(hash));
}

SingleInstance::SingleInstance(const QString& name, QObject* parent)
    : QObject(parent), m_name(name) {
    m_server.setSocketOptions(QLocalServer::UserAccessOption);
    connect(&m_server, &QLocalServer::newConnection, this, &SingleInstance::onNewConnection);
}

bool SingleInstance::forward(const QStringList& files, int timeoutMs) const {
    QLocalSocket socket;
    socket.connectToServer(m_name);
    if (!socket.waitForConnected(timeoutMs)) {
        return false;
    }
    QByteArray message;
    {
        QDataStream out(&message, QIODevice::WriteOnly);
        out.setVersion(QDataStream::Qt_6_0);
        out << kMagic << files.mid(0, kMaxFiles);
    }
    if (message.size() > kMaxMessageBytes) {
        message.clear();
        QDataStream out(&message, QIODevice::WriteOnly);
        out.setVersion(QDataStream::Qt_6_0);
        out << kMagic << QStringList();
    }
    socket.write(message);
    // The receiver answers one byte once it has read the message, so that the sender may exit
    // knowing it was taken.
    if (!socket.waitForBytesWritten(timeoutMs) || !socket.waitForReadyRead(timeoutMs)) {
        return false;
    }
    return socket.read(1) == "1";
}

bool SingleInstance::listen() {
    if (m_server.isListening()) {
        return true;
    }
    QLocalServer::removeServer(m_name);
    return m_server.listen(m_name);
}

void SingleInstance::onNewConnection() {
    while (QLocalSocket* socket = m_server.nextPendingConnection()) {
        socket->setParent(this);
        const auto buffer = QSharedPointer<QByteArray>::create();
        const auto done = QSharedPointer<bool>::create(false);
        auto* timer = new QTimer(socket);
        timer->setSingleShot(true);
        // A client that connects and goes quiet is dropped after a while.
        connect(timer, &QTimer::timeout, socket, [socket, done] {
            *done = true;
            socket->abort();
            socket->deleteLater();
        });
        timer->start(kReadTimeoutMs);
        connect(socket, &QLocalSocket::readyRead, this, [this, socket, buffer, done, timer] {
            if (*done) {
                return;
            }
            buffer->append(socket->readAll());
            if (buffer->size() > kMaxMessageBytes) {
                *done = true;
                socket->abort();
                socket->deleteLater();
                return;
            }
            QStringList files;
            quint32 magic = 0;
            QDataStream in(*buffer);
            in.setVersion(QDataStream::Qt_6_0);
            in.startTransaction();
            in >> magic >> files;
            if (!in.commitTransaction()) {
                return; // not all of it has arrived yet
            }
            *done = true;
            timer->stop();
            if (magic == kMagic) {
                socket->write("1");
                socket->flush();
                emit filesReceived(files.mid(0, kMaxFiles));
                socket->disconnectFromServer();
            } else {
                socket->abort();
            }
            socket->deleteLater();
        });
    }
}
} // namespace vellora