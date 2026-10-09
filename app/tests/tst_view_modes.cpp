// View modes in the window: the layout, rotation and zoom commands and their check marks, turning
// pages with the wheel and Page Down, the zoom rectangle, and what is remembered.
#include "MainWindow.h"

#include <QAction>
#include <QMenu>
#include <QMenuBar>
#include <QRubberBand>
#include <QScrollBar>
#include <QSettings>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QWheelEvent>

using vellora::CanvasController;
using vellora::PageLayout;

namespace {

constexpr int kWaitMs = 60'000;

// The window with the golden document open and its layout known.
struct Opened {
    vellora::MainWindow window;

    explicit Opened(vellora::AppSettings* settings = nullptr) : window(settings) {
        window.resize(700, 520);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)) || !opened.wait(kWaitMs)) {
            qFatal("could not open the golden document");
        }
    }

    CanvasController* controller() { return window.canvas().controller(); }
    bool run(const char* id) { return window.commands().run(QString::fromLatin1(id)); }
    QAction* action(const QString& title) {
        for (QAction* a : window.findChildren<QAction*>()) {
            if (a->text() == title) {
                return a;
            }
        }
        return nullptr;
    }
};

} // namespace

class TstViewModes : public QObject {
    Q_OBJECT

private slots:
    void theLayoutCommandsSetTheModeAndTheMenuShowsIt() {
        Opened o;
        const auto checked = [&](const QString& title) {
            QAction* a = o.action(title);
            return a != nullptr && a->isChecked();
        };
        // The start: continuous, one page a row.
        QVERIFY(checked(QStringLiteral("Continuous Scrolling")));
        QVERIFY(!checked(QStringLiteral("Single Page")));

        struct Row {
            const char* id;
            const char* title;
            bool continuous;
            PageLayout::Spread spread;
        };
        for (const Row& row :
             {Row{"view.layout.single", "Single Page", false, PageLayout::Spread::One},
              Row{"view.layout.twoUp", "Two Pages", false, PageLayout::Spread::Two},
              Row{"view.layout.twoUpContinuous", "Two Pages, Continuous", true,
                  PageLayout::Spread::Two},
              Row{"view.layout.continuous", "Continuous Scrolling", true,
                  PageLayout::Spread::One}}) {
            QVERIFY2(o.run(row.id), row.id);
            const auto mode = o.controller()->viewMode();
            QCOMPARE(mode.continuous, row.continuous);
            QCOMPARE(mode.spread, row.spread);
            // Exactly the matching entry is checked.
            for (const char* title :
                 {"Single Page", "Continuous Scrolling", "Two Pages", "Two Pages, Continuous"}) {
                QCOMPARE(checked(QString::fromLatin1(title)),
                         QString::fromLatin1(title) == QString::fromLatin1(row.title));
            }
        }
    }

    void theCoverPageOptionAppliesToTwoPageLayouts() {
        Opened o;
        QVERIFY(!o.action(QStringLiteral("Cover Page in Two-Page View"))->isChecked());
        // In a one-page layout it only sets what the two-page layouts will do.
        QVERIFY(o.run("view.layout.cover"));
        QVERIFY(o.action(QStringLiteral("Cover Page in Two-Page View"))->isChecked());
        QCOMPARE(o.controller()->viewMode().spread, PageLayout::Spread::One);
        QVERIFY(o.run("view.layout.twoUpContinuous"));
        QCOMPARE(o.controller()->viewMode().spread, PageLayout::Spread::TwoCover);
        // Cover page alone on the right of the spine.
        const vellora::Frame frame = o.controller()->frame();
        QCOMPARE(frame.pages.first().page, 0U);
        QVERIFY(frame.pages.first().rect.left() > o.window.canvas().viewport()->width() / 2.0);
        // And in a two-page layout it switches at once.
        QVERIFY(o.run("view.layout.cover"));
        QCOMPARE(o.controller()->viewMode().spread, PageLayout::Spread::Two);
        QVERIFY(!o.action(QStringLiteral("Cover Page in Two-Page View"))->isChecked());
        QVERIFY(o.run("view.layout.cover"));
        QCOMPARE(o.controller()->viewMode().spread, PageLayout::Spread::TwoCover);
        QVERIFY(o.run("view.layout.continuous"));
        QCOMPARE(o.controller()->viewMode().spread, PageLayout::Spread::One);
        QVERIFY(o.action(QStringLiteral("Cover Page in Two-Page View"))->isChecked());
    }

    void rotationAndZoomCommandsAreRegisteredWithTheirShortcuts() {
        Opened o;
        const auto shortcuts = [&](const char* id) {
            return o.window.commands().find(QString::fromLatin1(id))->shortcuts;
        };
        QCOMPARE(shortcuts("view.rotateClockwise"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::Key_BracketRight)}));
        QCOMPARE(shortcuts("view.rotateCounterclockwise"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::Key_BracketLeft)}));
        QCOMPARE(shortcuts("view.fitPage"),
                 (QList<QKeySequence>{QKeySequence(Qt::CTRL | Qt::Key_0)}));

        QVERIFY(o.run("view.rotateClockwise"));
        QCOMPARE(o.controller()->viewMode().rotation, 1);
        QVERIFY(o.run("view.rotateClockwise"));
        QCOMPARE(o.controller()->viewMode().rotation, 2);
        QVERIFY(o.run("view.rotateCounterclockwise"));
        QVERIFY(o.run("view.rotateCounterclockwise"));
        QVERIFY(o.run("view.rotateCounterclockwise"));
        QCOMPARE(o.controller()->viewMode().rotation, 3);

        // Every preset has its command and reaches its zoom, at the middle of the window.
        for (const double preset : CanvasController::zoomPresets()) {
            const QString id = QStringLiteral("view.zoom.%1").arg(qRound(preset * 100.0));
            QVERIFY2(o.window.commands().run(id), qPrintable(id));
            QCOMPARE(o.controller()->zoom(), preset);
            QCOMPARE(o.window.zoomStatus(), QStringLiteral("%1%").arg(qRound(preset * 100.0)));
        }
        QVERIFY(o.run("view.fitPage"));
        QCOMPARE(o.controller()->zoomMode(), CanvasController::ZoomMode::FitPage);
        QVERIFY(o.run("view.fitWidth"));
        QCOMPARE(o.controller()->zoomMode(), CanvasController::ZoomMode::FitWidth);

        // The zoom entries are in the menu: View > Zoom has the presets.
        QVERIFY(o.action(QStringLiteral("Zoom 6400%")) != nullptr);
        QVERIFY(o.action(QStringLiteral("Zoom 25%")) != nullptr);
    }

    void pageCommandsNavigate() {
        Opened o;
        // The middle of the window is on the second page, which stays current when the layout
        // changes; start from the first.
        o.controller()->goToPage(0);
        QVERIFY(o.run("view.layout.single"));
        o.controller()->goToPage(0);
        QCOMPARE(o.controller()->currentPage(), 0U);
        QVERIFY(o.run("view.nextPage"));
        QCOMPARE(o.controller()->currentPage(), 1U);
        QCOMPARE(o.window.pageStatus(), QStringLiteral("Page 2 / 3"));
        QVERIFY(o.run("view.previousPage"));
        QCOMPARE(o.controller()->currentPage(), 0U);
    }

    void theWheelAndPageDownTurnPagesWhenTheViewShowsOneRow() {
        Opened o;
        QVERIFY(o.run("view.layout.single"));
        // Zoomed in so that a row is clearly taller than the window and there is something to
        // scroll (a row that fits has no room, and every wheel notch would turn the page).
        o.controller()->setZoom(5.0, QPointF(0.0, 0.0));
        o.controller()->goToPage(0);
        QVERIFY(o.controller()->verticalRange().max > o.controller()->verticalRange().min + 100.0);
        QWidget* viewport = o.window.canvas().viewport();
        const auto wheel = [&](int notches) {
            QWheelEvent event(QPointF(100, 100), viewport->mapToGlobal(QPointF(100, 100)), QPoint(),
                              QPoint(0, notches * 120), Qt::NoButton, Qt::NoModifier,
                              Qt::NoScrollPhase, false);
            QCoreApplication::sendEvent(viewport, &event);
        };
        // The scroll bar spans the row, not the document.
        const auto range = o.controller()->verticalRange();
        QCOMPARE(o.window.canvas().verticalScrollBar()->minimum(), static_cast<int>(range.min));
        QCOMPARE(o.window.canvas().verticalScrollBar()->maximum(), static_cast<int>(range.max));

        wheel(-1); // down one notch scrolls within the page
        QVERIFY(o.controller()->scrollPosition().y() > range.min);
        QCOMPARE(o.controller()->currentPage(), 0U);
        for (int i = 0; i < 60 && o.controller()->currentPage() == 0U; ++i) {
            wheel(-5);
        }
        QCOMPARE(o.controller()->currentPage(), 1U); // the end of the page: the next one
        // Back up through the top turns back.
        for (int i = 0; i < 60 && o.controller()->currentPage() == 1U; ++i) {
            wheel(5);
        }
        QCOMPARE(o.controller()->currentPage(), 0U);
        // Page Down at the bottom of a page does the same.
        o.controller()->setScrollPosition(QPointF(0.0, 1.0e9));
        QTest::keyClick(&o.window.canvas(), Qt::Key_PageDown);
        QCOMPARE(o.controller()->currentPage(), 1U);
        QTest::keyClick(&o.window.canvas(), Qt::Key_PageUp);
        QCOMPARE(o.controller()->currentPage(), 0U);
        // In continuous scrolling the wheel is the scroll area's: no page is turned by it.
        QVERIFY(o.run("view.layout.continuous"));
        const double before = o.controller()->scrollPosition().y();
        wheel(-1);
        QVERIFY(o.controller()->scrollPosition().y() >= before);
    }

    void dragWhileHoldingZZoomsToTheRectangle() {
        Opened o;
        vellora::CanvasView& view = o.window.canvas();
        view.setFocus();
        QWidget* viewport = view.viewport();
        QVERIFY(!view.zoomRectArmed());
        const double zoom = o.controller()->zoom();

        // A drag without Z scrolls nothing and zooms nothing.
        QTest::mousePress(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(100, 60));
        QTest::mouseRelease(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(200, 120));
        QCOMPARE(o.controller()->zoom(), zoom);

        QTest::keyPress(&view, Qt::Key_Z);
        QVERIFY(view.zoomRectArmed());
        QTest::mousePress(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(240, 40));
        QTest::mouseMove(viewport, QPoint(300, 80));
        QVERIFY(view.dragRect().isValid());
        QCOMPARE(view.dragRect(), QRect(QPoint(240, 40), QPoint(300, 80)));
        QTest::mouseRelease(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(340, 80));
        QTest::keyRelease(&view, Qt::Key_Z);
        QVERIFY(!view.zoomRectArmed());
        QVERIFY(view.dragRect().isEmpty());
        // 100 x 40 pixels filled the window: the zoom grew by at least a factor of three.
        QVERIFY2(o.controller()->zoom() > 3.0 * zoom,
                 qPrintable(QString::number(o.controller()->zoom())));
        QCOMPARE(o.controller()->zoomMode(), CanvasController::ZoomMode::Custom);

        // A click with Z held is not a rectangle.
        o.controller()->setZoom(1.0, QPointF(0.0, 0.0));
        QTest::keyPress(&view, Qt::Key_Z);
        QTest::mouseClick(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(150, 100));
        QTest::keyRelease(&view, Qt::Key_Z);
        QCOMPARE(o.controller()->zoom(), 1.0);
    }

    void theCommandArmsTheRectangleForOneDragAndEscapeCancels() {
        Opened o;
        vellora::CanvasView& view = o.window.canvas();
        view.setFocus();
        QWidget* viewport = view.viewport();
        QVERIFY(o.run("view.zoomToSelection"));
        QVERIFY(view.zoomRectArmed());
        // Escape disarms it.
        QTest::keyClick(&view, Qt::Key_Escape);
        QVERIFY(!view.zoomRectArmed());

        QVERIFY(o.run("view.zoomToSelection"));
        QTest::mousePress(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(240, 40));
        QTest::mouseMove(viewport, QPoint(280, 70));
        QTest::mouseRelease(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(300, 80));
        QVERIFY(o.controller()->zoom() > 1.0);
        QVERIFY(!view.zoomRectArmed()); // one drag
        // Escape in the middle of a drag cancels it without zooming.
        o.controller()->setZoom(1.0, QPointF(0.0, 0.0));
        QVERIFY(o.run("view.zoomToSelection"));
        QTest::mousePress(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(240, 40));
        QTest::mouseMove(viewport, QPoint(300, 90));
        QVERIFY(view.dragRect().isValid());
        QTest::keyClick(&view, Qt::Key_Escape);
        QVERIFY(view.dragRect().isEmpty());
        QTest::mouseRelease(viewport, Qt::LeftButton, Qt::NoModifier, QPoint(300, 90));
        QCOMPARE(o.controller()->zoom(), 1.0);
    }

    void theArrangementIsRememberedPerDocumentAndAsTheLastChoice() {
        QTemporaryDir dir;
        const QString ini = dir.filePath(QStringLiteral("s.ini"));
        const QString copy = dir.filePath(QStringLiteral("copy.pdf"));
        QVERIFY(QFile::copy(QStringLiteral(VELLORA_GOLDEN_PDF), copy));
        {
            QSettings backing(ini, QSettings::IniFormat);
            vellora::AppSettings settings(&backing);
            vellora::MainWindow window(&settings);
            window.resize(700, 520);
            window.show();
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(copy));
            QVERIFY(opened.wait(kWaitMs));
            // The user chooses two pages with a cover, continuous, turned once.
            QVERIFY(window.commands().run(QStringLiteral("view.layout.cover")));
            QVERIFY(window.commands().run(QStringLiteral("view.layout.twoUpContinuous")));
            QVERIFY(window.commands().run(QStringLiteral("view.rotateClockwise")));
            window.closeTab(0);
            QCOMPARE(settings.lastLayout(), (vellora::AppSettings::Layout{true, 2, 1}));
        }
        QSettings backing(ini, QSettings::IniFormat);
        vellora::AppSettings settings(&backing);
        // The document comes back arranged as it was left, whatever the user chose last for
        // other documents...
        const vellora::AppSettings::Layout lastChoice{false, 0, 0};
        settings.setLastLayout(lastChoice);
        {
            vellora::MainWindow window(&settings);
            window.resize(700, 520);
            window.show();
            QCOMPARE(window.canvas().controller()->viewMode().spread, PageLayout::Spread::One);
            QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
            QVERIFY(window.openDocument(copy));
            QVERIFY(opened.wait(kWaitMs));
            const auto mode = window.canvas().controller()->viewMode();
            QVERIFY(mode.continuous);
            QCOMPARE(mode.spread, PageLayout::Spread::TwoCover);
            QCOMPARE(mode.rotation, 1);
        }
        // ...and restoring it did not become the user's choice.
        QCOMPARE(settings.lastLayout(), lastChoice);

        // A document that was never opened starts as the user last chose.
        settings.setLastLayout({true, 2, 1});
        const QString other = dir.filePath(QStringLiteral("other.pdf"));
        QVERIFY(QFile::copy(QStringLiteral(VELLORA_GOLDEN_PDF), other));
        vellora::MainWindow window(&settings);
        window.resize(700, 520);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(other));
        QVERIFY(opened.wait(kWaitMs));
        const auto mode = window.canvas().controller()->viewMode();
        QCOMPARE(mode.spread, PageLayout::Spread::TwoCover);
        QCOMPARE(mode.rotation, 1);
        // The menu shows it.
        QAction* twoContinuous = nullptr;
        for (QAction* a : window.findChildren<QAction*>()) {
            if (a->text() == QStringLiteral("Two Pages, Continuous")) {
                twoContinuous = a;
            }
        }
        QVERIFY(twoContinuous != nullptr && twoContinuous->isChecked());
    }
};

QTEST_MAIN(TstViewModes)
#include "tst_view_modes.moc"