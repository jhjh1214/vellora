// EngineSession against the real engine: open, page sizes, a tile through the bridge's cache.
#include "bridge/EngineSession.h"

#include <QSignalSpy>
#include <QTest>

namespace {

constexpr int kWaitMs = 60'000;

} // namespace

class TstEngineSession : public QObject {
    Q_OBJECT

private slots:
    void tileGeometry() {
        QCOMPARE(vellora::EngineSession::tilePixels(), 512U);
        QCOMPARE(vellora::EngineSession::bucketScale(1.0F), 1.0F);
        QCOMPARE(vellora::EngineSession::bucketScale(0.0F), 0.0F);
    }

    void closedSessionAnswersWithoutFailing() {
        vellora::EngineSession session;
        QVERIFY(!session.isOpen());
        QCOMPARE(session.pageCount(), 0U);
        QVERIFY(!session.pageSize(0).isValid());
        QByteArray tile;
        QVERIFY(!session.readTile(0, 1.0F, 0, 0, tile));
        QCOMPARE(session.requestTile(0, 1.0F, 0, 0, vellora::TilePriority::Visible).state,
                 vellora::TileState::Full);
    }

    void missingFileIsReported() {
        vellora::EngineSession session;
        QVERIFY(!session.open(QStringLiteral("this/file/does/not/exist.pdf")).isEmpty());
        QVERIFY(!session.isOpen());
    }

    void opensAndServesATile() {
        vellora::EngineSession session;
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QSignalSpy tileReady(&session, &vellora::EngineSession::tileReady);

        QCOMPARE(session.open(QStringLiteral(VELLORA_GOLDEN_PDF)), QString());
        QVERIFY(session.isOpen());
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(opened.first().at(0).toUInt(), 3U);
        QVERIFY(opened.first().at(1).toStringList().isEmpty());
        QCOMPARE(session.pageCount(), 3U);
        QCOMPARE(session.pageSize(0), QSizeF(220.0, 140.0));
        QVERIFY(!session.pageSize(99).isValid());

        // A miss goes to the engine; the tile is readable once its answer arrived.
        const vellora::TileTicket ticket =
            session.requestTile(0, 1.0F, 0, 0, vellora::TilePriority::Visible);
        QCOMPARE(ticket.state, vellora::TileState::Requested);
        QByteArray tile;
        QVERIFY(!session.readTile(0, 1.0F, 0, 0, tile));
        QVERIFY(tileReady.wait(kWaitMs));
        QCOMPARE(tileReady.first().at(0).toULongLong(), static_cast<quint64>(ticket.request));

        QVERIFY(session.readTile(0, 1.0F, 0, 0, tile));
        QCOMPARE(static_cast<quint32>(tile.size()), session.slotBytes());
        // Page 0 has content; BGRx, so a pixel that is not white has some byte below 0xFF.
        bool hasContent = false;
        const qsizetype used = qsizetype{512} * 512 * 4;
        for (qsizetype i = 0; i < used && !hasContent; ++i) {
            hasContent = static_cast<uchar>(tile.at(i)) != 0xFF;
        }
        QVERIFY(hasContent);

        // The second request is a cache hit: nothing is sent, so no further answer arrives.
        QCOMPARE(session.requestTile(0, 1.0F, 0, 0, vellora::TilePriority::Visible).state,
                 vellora::TileState::Ready);
        session.invalidatePage(0);
        QVERIFY(!session.readTile(0, 1.0F, 0, 0, tile));

        session.close();
        QVERIFY(!session.isOpen());
    }

    void anEncryptedDocumentAsksForItsPasswordThenOpens() {
        vellora::EngineSession session;
        QSignalSpy opened(&session, &vellora::EngineSession::opened);
        QSignalSpy asked(&session, &vellora::EngineSession::passwordRequested);
        QSignalSpy failed(&session, &vellora::EngineSession::requestFailed);

        QCOMPARE(session.open(QStringLiteral(VELLORA_PROTECTED_PDF)), QString());
        QVERIFY(asked.wait(kWaitMs));
        QCOMPARE(asked.size(), 1);
        QVERIFY(!asked.at(0).at(0).toBool()); // required, not "wrong"
        QCOMPARE(opened.size(), 0);
        QCOMPARE(session.pageCount(), 0U);

        // Wrong passwords are refused as many times as they are sent; the engine stays up.
        for (int attempt = 0; attempt < 3; ++attempt) {
            asked.clear();
            QCOMPARE(session.submitPassword(QStringLiteral("wrong")), QString());
            QVERIFY(asked.wait(kWaitMs));
            QVERIFY(asked.at(0).at(0).toBool());
            QCOMPARE(opened.size(), 0);
        }
        // A password the protocol cannot carry is refused at once and changes nothing.
        QVERIFY(!session.submitPassword(QString::fromUtf8("nul\0inside", 10)).isEmpty());

        QCOMPARE(session.submitPassword(QStringLiteral("user-pw")), QString());
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(opened.first().at(0).toUInt(), 1U);
        QCOMPARE(session.pageCount(), 1U);
        // Neither answer was reported as an ordinary failure.
        QCOMPARE(failed.size(), 0);
        // The document is open now: a password is no longer accepted.
        QVERIFY(!session.submitPassword(QStringLiteral("user-pw")).isEmpty());
    }

    void submittingAPasswordToAClosedSessionIsAnError() {
        vellora::EngineSession session;
        QVERIFY(!session.submitPassword(QStringLiteral("x")).isEmpty());
    }
};

QTEST_MAIN(TstEngineSession)
#include "tst_engine_session.moc"
