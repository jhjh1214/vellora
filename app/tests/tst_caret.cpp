// M1 task 17b: caret browsing. F7 puts a text cursor in the document that the keyboard moves by
// character, word and line across pages, and that selects with Shift. A real engine and no GPU;
// the document is four small pages of Helvetica text, the third without any.
#include "MainWindow.h"
#include "TextPdf.h"
#include "VelloraTestMain.h"
#include "accessibility/CanvasAccessible.h"
#include "canvas/CanvasController.h"
#include "text/CaretBrowser.h"
#include "text/SelectionOverlay.h"

#include <QAccessible>
#include <QAccessibleTextInterface>
#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <cmath>

using vellora::CanvasView;
using vellora::DocumentTab;
using Position = vellora::TextSelection::Position;

namespace {

constexpr int kWaitMs = 60'000;

QString line(const QString& text, int y) {
    return QStringLiteral("BT /F1 20 Tf 72 %1 Td (%2) Tj ET").arg(y).arg(text);
}

// Line one is "alpha beta gamma" and line two "delta epsilon", each followed by the line break
// PDFium inserts (two characters), so line two starts at character 18.
constexpr int kSecondLine = 18;
constexpr int kSecondLineLength = 13;

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    DocumentTab* tab = nullptr;

    Fixture() {
        const QStringList pages = {
            line(QStringLiteral("alpha beta gamma"), 700) + QLatin1Char(' ') +
                line(QStringLiteral("delta epsilon"), 660) + QLatin1Char(' ') +
                line(QStringLiteral("zeta"), 620),
            line(QStringLiteral("second page"), 700),
            QStringLiteral("q Q"),
            line(QStringLiteral("final"), 700),
        };
        QFile file(dir.filePath(QStringLiteral("caret.pdf")));
        if (!file.open(QIODevice::WriteOnly)) {
            qFatal("cannot write the test document");
        }
        file.write(textPdf(pages));
        file.close();
        window.resize(900, 900);
        window.show();
        tab = &window.currentTab();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(dir.filePath(QStringLiteral("caret.pdf"))) ||
            !opened.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
        view().controller()->setZoom(0.6, QPointF(0.0, 0.0));
        view().controller()->goToPage(0);
    }

    CanvasView& view() { return window.canvas(); }
    vellora::CaretBrowser* caret() { return view().caret(); }

    // Turns browsing on once the text of the first page is here.
    void browse() {
        requireText(0);
        tab->toggleCaretBrowsing();
        QVERIFY(caret()->isOn());
    }

    void requireText(quint32 page) {
        if (!textArrived(page)) {
            qFatal("the text of the page did not arrive");
        }
    }

    bool textArrived(quint32 page) {
        view().text()->ensure(page);
        return QTest::qWaitFor([&] { return view().text()->isComplete(page); }, kWaitMs);
    }

    void key(Qt::Key key, Qt::KeyboardModifiers modifiers = Qt::NoModifier) {
        QTest::keyClick(&view(), key, modifiers);
    }

    // Where the caret is, once it has stopped moving for pages that have to be fetched.
    int at() const { return currentPosition().caret; }
    Position currentPosition() const {
        const auto position = const_cast<Fixture*>(this)->caret()->position();
        return position ? *position : Position{9999, 9999};
    }

    // The selected text (empty if none), read from the engine.
    QString selectedText() {
        QString text;
        bool done = false;
        view().selection()->copyText([&](const QString& t) {
            text = t;
            done = true;
        });
        static_cast<void>(QTest::qWaitFor([&] { return done; }, kWaitMs));
        return text;
    }
};

} // namespace

class TstCaret : public QObject {
    Q_OBJECT

private slots:
    void browsingIsOffUntilF7AndTheKeysScrollAsBefore() {
        Fixture f;
        QVERIFY(!f.caret()->isOn());
        QVERIFY(!f.caret()->position());
        QVERIFY(!f.tab->caretBrowsing());
        QVERIFY(f.view().selectionOverlay()->caretRect().isEmpty());
        f.key(Qt::Key_Right);
        QVERIFY(!f.caret()->position());

        f.browse();
        QCOMPARE(f.currentPosition(), (Position{0, 0}));
        QVERIFY(f.tab->caretBrowsing());
        f.tab->toggleCaretBrowsing();
        QVERIFY(!f.caret()->isOn());
        QVERIFY(!f.caret()->position());
    }

    void theCaretIsPaintedWhileBrowsing() {
        Fixture f;
        f.browse();
        QTRY_VERIFY_WITH_TIMEOUT(!f.view().selectionOverlay()->caretRect().isEmpty(), kWaitMs);
        // At the start of the first character of the first line.
        const QRectF caret = f.view().selectionOverlay()->caretRect();
        QVERIFY(caret.height() > 5.0);
        f.key(Qt::Key_Right);
        QTRY_VERIFY_WITH_TIMEOUT(f.view().selectionOverlay()->caretRect().left() > caret.left(),
                                 kWaitMs);
        f.tab->toggleCaretBrowsing();
        QVERIFY(f.view().selectionOverlay()->caretRect().isEmpty());
    }

    void leftAndRightMoveByCharacter() {
        Fixture f;
        f.browse();
        for (int i = 0; i < 3; ++i) {
            f.key(Qt::Key_Right);
        }
        QCOMPARE(f.at(), 3);
        f.key(Qt::Key_Left);
        QCOMPARE(f.at(), 2);
        // Not past the start of the document.
        f.key(Qt::Key_Left);
        f.key(Qt::Key_Left);
        f.key(Qt::Key_Left);
        QCOMPARE(f.currentPosition(), (Position{0, 0}));
    }

    void controlMovesByWord() {
        Fixture f;
        f.browse();
        f.key(Qt::Key_Right, Qt::ControlModifier);
        QCOMPARE(f.at(), 6); // after "alpha "
        f.key(Qt::Key_Right, Qt::ControlModifier);
        QCOMPARE(f.at(), 11); // after "beta "
        f.key(Qt::Key_Left, Qt::ControlModifier);
        QCOMPARE(f.at(), 6);
        f.key(Qt::Key_Left, Qt::ControlModifier);
        QCOMPARE(f.at(), 0);
    }

    void upAndDownMoveByLineKeepingTheColumn() {
        Fixture f;
        f.browse();
        for (int i = 0; i < 3; ++i) {
            f.key(Qt::Key_Right);
        }
        f.key(Qt::Key_Down);
        // Three characters in on the second line, give or take the difference in widths.
        QVERIFY2(std::abs(f.at() - (kSecondLine + 3)) <= 1, qPrintable(QString::number(f.at())));
        const int second = f.at();
        f.key(Qt::Key_Down);
        QVERIFY(f.at() > second);
        f.key(Qt::Key_Up);
        f.key(Qt::Key_Up);
        QVERIFY2(std::abs(f.at() - 3) <= 1, qPrintable(QString::number(f.at())));
        // At the first line of the document, Up stays.
        f.key(Qt::Key_Up);
        QCOMPARE(f.currentPosition().page, 0U);
        QVERIFY(f.at() <= 4);
    }

    void homeAndEndGoToTheEdgesOfTheLine() {
        Fixture f;
        f.browse();
        f.key(Qt::Key_Down);
        f.key(Qt::Key_End);
        QCOMPARE(f.at(), kSecondLine + kSecondLineLength);
        // End again stays on the line (the break is not part of it).
        f.key(Qt::Key_End);
        QCOMPARE(f.at(), kSecondLine + kSecondLineLength);
        f.key(Qt::Key_Home);
        QCOMPARE(f.at(), kSecondLine);
        f.key(Qt::Key_Home);
        QCOMPARE(f.at(), kSecondLine);
    }

    void controlHomeAndEndGoToTheEdgesOfTheDocument() {
        Fixture f;
        f.browse();
        f.key(Qt::Key_End, Qt::ControlModifier);
        QTRY_COMPARE_WITH_TIMEOUT(f.currentPosition().page, 3U, kWaitMs);
        QVERIFY2(f.at() >= 5, qPrintable(QString::number(f.at()))); // after "final"
        f.key(Qt::Key_Home, Qt::ControlModifier);
        QCOMPARE(f.currentPosition(), (Position{0, 0}));
    }

    void theCaretCrossesPagesAndSkipsPagesWithoutText() {
        Fixture f;
        f.browse();
        // To the end of page one, then on to page two.
        f.key(Qt::Key_Down);
        f.key(Qt::Key_Down);
        f.key(Qt::Key_End);
        const int lastOfFirst = f.at();
        // (The text of a page may end with a line break, which takes a press or two to cross.)
        for (int i = 0; i < 4 && f.currentPosition().page == 0; ++i) {
            f.key(Qt::Key_Right);
            QTest::qWait(100);
        }
        QTRY_COMPARE_WITH_TIMEOUT(f.currentPosition().page, 1U, kWaitMs);
        QCOMPARE(f.at(), 0);
        QVERIFY(lastOfFirst > 0);
        // From the end of page two, the page without text is skipped.
        f.key(Qt::Key_End);
        for (int i = 0; i < 4 && f.currentPosition().page == 1; ++i) {
            f.key(Qt::Key_Right);
            QTest::qWait(100);
        }
        QTRY_COMPARE_WITH_TIMEOUT(f.currentPosition().page, 3U, kWaitMs);
        QCOMPARE(f.at(), 0);
        // And back again.
        f.key(Qt::Key_Left);
        QTRY_COMPARE_WITH_TIMEOUT(f.currentPosition().page, 1U, kWaitMs);
        QVERIFY(f.at() > 0);
        // Down from the last line of a page goes to the first line of the next one with text.
        f.key(Qt::Key_Down);
        QTRY_COMPARE_WITH_TIMEOUT(f.currentPosition().page, 3U, kWaitMs);
    }

    void shiftSelectsFromWhereTheCaretWas() {
        Fixture f;
        f.browse();
        for (int i = 0; i < 5; ++i) {
            f.key(Qt::Key_Right, Qt::ShiftModifier);
        }
        QCOMPARE(f.at(), 5);
        QCOMPARE(f.selectedText(), QStringLiteral("alpha"));
        // By word, then back by character: the anchor stays at the start.
        f.key(Qt::Key_Right, Qt::ShiftModifier | Qt::ControlModifier);
        QCOMPARE(f.selectedText(), QStringLiteral("alpha "));
        f.key(Qt::Key_Left, Qt::ShiftModifier);
        QCOMPARE(f.selectedText(), QStringLiteral("alpha"));
        // Down by a line takes the text of the lines between.
        f.key(Qt::Key_Down, Qt::ShiftModifier);
        QVERIFY(f.selectedText().startsWith(QStringLiteral("alpha beta gamma\ndelta")));
        // Without Shift the selection goes and the caret goes on from its end.
        f.key(Qt::Key_Right);
        QVERIFY(f.view().selection()->isEmpty());
        // Shifting back over the start selects the other way.
        f.key(Qt::Key_Home, Qt::ControlModifier);
        for (int i = 0; i < 3; ++i) {
            f.key(Qt::Key_Right);
        }
        for (int i = 0; i < 2; ++i) {
            f.key(Qt::Key_Left, Qt::ShiftModifier);
        }
        QCOMPARE(f.selectedText(), QStringLiteral("lp"));
    }

    void browsingStartsAtTheEndOfTheSelection() {
        Fixture f;
        f.requireText(0);
        f.view().selection()->selectPage(0);
        f.tab->toggleCaretBrowsing();
        QTRY_VERIFY_WITH_TIMEOUT(f.caret()->position().has_value(), kWaitMs);
        QCOMPARE(f.currentPosition().page, 0U);
        // The end of the page's text: line two's end plus line three ("zeta") and its break.
        QVERIFY2(f.at() >= kSecondLine + kSecondLineLength + 2 + 4,
                 qPrintable(QString::number(f.at())));
    }

    void aClickPutsTheCaretWhereItLands() {
        Fixture f;
        f.browse();
        f.key(Qt::Key_End);
        const int end = f.at();
        QVERIFY(end > 5);
        f.caret()->placeAt(0, QPointF(80.0, 90.0)); // near the left end of the first line
        QVERIFY2(f.at() < end, qPrintable(QString::number(f.at())));
    }

    void theScreenReaderIsToldWhereTheCaretIs() {
        Fixture f;
        f.browse();
        for (int i = 0; i < 4; ++i) {
            f.key(Qt::Key_Right);
        }
        QAccessibleInterface* page = vellora::canvasPageInterface(&f.view(), 0);
        QVERIFY(page != nullptr);
        QCOMPARE(page->textInterface()->cursorPosition(), 4);
        // A page the caret is not on has none.
        QCOMPARE(vellora::canvasPageInterface(&f.view(), 1)->textInterface()->cursorPosition(), 0);
        // The reader can move it, too.
        page->textInterface()->setCursorPosition(8);
        QCOMPARE(f.at(), 8);
    }

    void aNewDocumentTurnsBrowsingOff() {
        Fixture f;
        f.browse();
        QTemporaryDir other;
        QFile file(other.filePath(QStringLiteral("other.pdf")));
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(textPdf({line(QStringLiteral("other"), 700)}));
        file.close();
        QSignalSpy opened(&f.window.session(), &vellora::EngineSession::opened);
        QVERIFY(f.tab->open(other.filePath(QStringLiteral("other.pdf"))));
        QVERIFY(opened.wait(kWaitMs));
        QVERIFY(!f.caret()->isOn());
        QVERIFY(!f.caret()->position());
    }
};

VELLORA_TEST_MAIN(TstCaret)
#include "tst_caret.moc"
