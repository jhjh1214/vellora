// SingleInstance: a second launch hands its files to the running instance.
#include "SingleInstance.h"

#include <QCoreApplication>
#include <QLocalSocket>
#include <QSignalSpy>
#include <QTest>
#include <QThread>
#include <memory>

namespace {

// A name no other test run (or a real Vellora) uses.
QString uniqueName(const char* test) {
    return QStringLiteral("vellora-test-%1-%2")
        .arg(QCoreApplication::applicationPid())
        .arg(QString::fromLatin1(test));
}

// Runs `forward` on another thread, as a second process would, while this thread's event loop
// serves the receiving instance.
bool forwardFromAnotherThread(const vellora::SingleInstance& sender, const QStringList& files) {
    bool result = false;
    std::unique_ptr<QThread> thread(QThread::create([&] { result = sender.forward(files); }));
    thread->start();
    const bool finished = QTest::qWaitFor([&] { return thread->isFinished(); }, 10'000);
    Q_UNUSED(finished);
    thread->wait();
    return result;
}

} // namespace

class TstSingleInstance : public QObject {
    Q_OBJECT

private slots:
    void nobodyListeningMeansThisIsTheFirstInstance() {
        const vellora::SingleInstance instance(uniqueName("nobody"));
        QVERIFY(!instance.forward({QStringLiteral("/a.pdf")}, 300));
    }

    void aSecondLaunchForwardsItsFilesToTheFirst() {
        const QString name = uniqueName("forward");
        vellora::SingleInstance first(name);
        QVERIFY(first.listen());
        QVERIFY(first.isListening());
        QSignalSpy received(&first, &vellora::SingleInstance::filesReceived);

        const vellora::SingleInstance second(name);
        const QStringList files = {QStringLiteral("/tmp/a.pdf"), QStringLiteral("C:/x/é ü.pdf")};
        QVERIFY(forwardFromAnotherThread(second, files));
        QTRY_COMPARE_WITH_TIMEOUT(received.size(), 1, 5'000);
        QCOMPARE(received.first().at(0).toStringList(), files);

        // A launch without files only wants the window in front.
        QVERIFY(forwardFromAnotherThread(second, {}));
        QTRY_COMPARE_WITH_TIMEOUT(received.size(), 2, 5'000);
        QVERIFY(received.last().at(0).toStringList().isEmpty());
    }

    void aMessageCarriesNoMoreThanTheCap() {
        const QString name = uniqueName("cap");
        vellora::SingleInstance first(name);
        QVERIFY(first.listen());
        QSignalSpy received(&first, &vellora::SingleInstance::filesReceived);
        QStringList many;
        for (int i = 0; i < 200; ++i) {
            many.append(QStringLiteral("/f%1.pdf").arg(i));
        }
        const vellora::SingleInstance second(name);
        QVERIFY(forwardFromAnotherThread(second, many));
        QTRY_COMPARE_WITH_TIMEOUT(received.size(), 1, 5'000);
        QCOMPARE(received.first().at(0).toStringList().size(), vellora::SingleInstance::kMaxFiles);
        QCOMPARE(received.first().at(0).toStringList().first(), QStringLiteral("/f0.pdf"));
    }

    void garbageAndOversizeMessagesAreDroppedWithoutHarm() {
        const QString name = uniqueName("garbage");
        vellora::SingleInstance first(name);
        QVERIFY(first.listen());
        QSignalSpy received(&first, &vellora::SingleInstance::filesReceived);

        for (const QByteArray& junk :
             {QByteArray("not a message at all"), QByteArray(4, '\0'),
              QByteArray(vellora::SingleInstance::kMaxMessageBytes + 4096, 'x')}) {
            QLocalSocket socket;
            socket.connectToServer(name);
            QVERIFY(socket.waitForConnected(2'000));
            socket.write(junk);
            socket.waitForBytesWritten(2'000);
            // Let the receiver look at it; it must neither answer "taken" nor emit.
            QTest::qWait(100);
            QVERIFY(socket.bytesAvailable() == 0 || socket.readAll() != "1");
        }
        QCOMPARE(received.size(), 0);

        // And it still serves a proper launch afterwards.
        const vellora::SingleInstance second(name);
        QVERIFY(forwardFromAnotherThread(second, {QStringLiteral("/ok.pdf")}));
        QTRY_COMPARE_WITH_TIMEOUT(received.size(), 1, 5'000);
    }

    void aLeftOverSocketIsReplacedWhenNobodyAnswers() {
        const QString name = uniqueName("stale");
        {
            vellora::SingleInstance gone(name);
            QVERIFY(gone.listen());
        }
        const vellora::SingleInstance probe(name);
        QVERIFY(!probe.forward({}, 300)); // nobody is there any more
        vellora::SingleInstance next(name);
        QVERIFY(next.listen());
    }
};

QTEST_MAIN(TstSingleInstance)
#include "tst_single_instance.moc"