// DiagnosticsScript: the scripted scroll and zoom does what it says, on a real engine and no GPU.
#include "SyntheticPdf.h"
#include "diagnostics/DiagnosticsScript.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

using vellora::CanvasController;
using vellora::DiagnosticsScript;
using vellora::EngineSession;

namespace {

constexpr int kWaitMs = 120'000;

struct Fixture {
    EngineSession session;
    CanvasController controller{&session};

    explicit Fixture(const QString& path) {
        controller.setViewportSize(QSize(900, 700));
        QSignalSpy opened(&session, &EngineSession::opened);
        const QString error = session.open(path);
        if (!error.isEmpty() || !opened.wait(kWaitMs)) {
            qFatal("could not open %s: %s", qPrintable(path), qPrintable(error));
        }
    }
};

} // namespace

class TstDiagnosticsScript : public QObject {
    Q_OBJECT

private slots:
    void scrollsTwoThousandPagesAndZoomsTwentyTimes() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("10k.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(10'000));
        file.close();

        Fixture f(path);
        QCOMPARE(f.controller.pageCount(), 10'000U);
        const double zoomBefore = f.controller.zoom();
        QSignalSpy zoomed(&f.controller, &CanvasController::zoomChanged);
        DiagnosticsScript script(&f.controller);
        QSignalSpy finished(&script, &DiagnosticsScript::finished);
        script.start();
        QVERIFY(script.running());
        QVERIFY(finished.wait(kWaitMs));

        QVERIFY(!script.running());
        QCOMPARE(script.stepsDone(), 2000);
        QCOMPARE(script.zoomsDone(), 20);
        QCOMPARE(zoomed.size(), 20);
        // Ten zooms in and ten out cancel (up to rounding), and the view has moved 2,000 pages
        // down: it shows the page with (zero-based) index 2000.
        QVERIFY(qAbs(f.controller.zoom() - zoomBefore) < 1e-9);
        QCOMPARE(f.controller.currentPage(), 2000U);
    }

    void aShortDocumentStillGetsEveryZoom() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF)); // three pages
        DiagnosticsScript script(&f.controller);
        QSignalSpy finished(&script, &DiagnosticsScript::finished);
        script.start();
        QVERIFY(finished.wait(kWaitMs));
        QCOMPARE(script.stepsDone(), 2);
        QCOMPARE(script.zoomsDone(), 20);
    }

    void aSinglePageDocumentFinishesAtOnce() {
        QTemporaryDir dir;
        QVERIFY(dir.isValid());
        const QString path = dir.filePath(QStringLiteral("one.pdf"));
        QFile file(path);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(syntheticPdf(1));
        file.close();

        Fixture f(path);
        DiagnosticsScript script(&f.controller);
        QSignalSpy finished(&script, &DiagnosticsScript::finished);
        script.start();
        QVERIFY(finished.wait(kWaitMs));
        QCOMPARE(script.stepsDone(), 0);
    }

    void onlyTheNamedScenarioExists() {
        QVERIFY(DiagnosticsScript::isScenario(QStringLiteral("scroll-zoom")));
        QVERIFY(!DiagnosticsScript::isScenario(QStringLiteral("")));
        QVERIFY(!DiagnosticsScript::isScenario(QStringLiteral("scroll")));
    }
};

QTEST_MAIN(TstDiagnosticsScript)
#include "tst_diagnostics_script.moc"
