// M1 task 14b: selecting and copying text, with a real engine and no GPU. The documents are small
// synthetic files with Helvetica text at known places. The golden file lists the rectangles the
// selection highlights (points of the page as shown); it is regenerated with
// VELLORA_UPDATE_GOLDEN=1 and the change must be read before it is committed.
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "TextPdf.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"
#include "text/SelectionOverlay.h"

#include <QApplication>
#include <QClipboard>
#include <QDir>
#include <QFile>
#include <QGuiApplication>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <cmath>

using vellora::PageText;
using vellora::TextChar;
using vellora::TextSelection;

namespace {

constexpr int kWaitMs = 60'000;
// The fixtures use the base-14 Helvetica, which each platform replaces with its own font, so the
// boxes differ by a fraction of a point from one machine to the next (0.3 pt between Windows and
// Linux). A selection that is wrong moves a rectangle by a whole character, at least 4 points.
constexpr double kGoldenTolerance = 1.0;

const QString kThreeLines = QStringLiteral(
    "BT /F1 20 Tf 72 700 Td (Hello World) Tj 0 -30 Td (Second line of text) Tj 0 -30 Td "
    "(Third line) Tj ET");
const QString kAlpha = QStringLiteral("BT /F1 20 Tf 72 700 Td (Alpha beta) Tj ET");
const QString kGamma = QStringLiteral("BT /F1 20 Tf 72 700 Td (Gamma delta) Tj ET");

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    vellora::DocumentTab* tab = nullptr;

    explicit Fixture(const QByteArray& pdf, int zoomPercent = 100) {
        const QString path = dir.filePath(QStringLiteral("text.pdf"));
        QFile file(path);
        if (!file.open(QIODevice::WriteOnly)) {
            qFatal("cannot write the test document");
        }
        file.write(pdf);
        file.close();
        window.resize(900, 900);
        window.show();
        tab = &window.currentTab();
        QSignalSpy openedSpy(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(path) || !openedSpy.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
        controller()->setZoom(zoomPercent / 100.0, QPointF(0.0, 0.0));
        controller()->goToPage(0);
        if (!QTest::qWaitFor([this] { return text()->isComplete(0); }, kWaitMs)) {
            qFatal("the text of page 1 did not arrive");
        }
    }

    vellora::CanvasController* controller() { return window.canvas().controller(); }
    vellora::TextLayer* text() { return window.canvas().text(); }
    vellora::TextSelection* selection() { return window.canvas().selection(); }
    vellora::CanvasView& view() { return window.canvas(); }
    QWidget* viewport() { return view().viewport(); }

    const PageText& page(quint32 index = 0) { return *text()->page(index); }

    // The place on screen (viewport pixels) of a point of a page as shown.
    QPoint at(quint32 page, QPointF point) {
        for (const vellora::PageDraw& draw : controller()->visiblePages()) {
            if (draw.page == page) {
                return draw.map(QRectF(point, QSizeF())).topLeft().toPoint();
            }
        }
        qFatal("page %u is not on screen", page);
        return {};
    }

    // The index of the first character of `needle` in the text of `pageIndex`.
    qsizetype find(const QString& needle, quint32 pageIndex = 0) {
        QString all;
        for (const TextChar& c : page(pageIndex).chars) {
            all.append(QString::fromUcs4(&c.ch, 1));
        }
        return all.indexOf(needle);
    }

    // A point near one end of character `index` along its line, in points of the page as shown:
    // `leading` is the end the reader reaches first. Well inside the half of the box where the
    // caret should fall (pixels are whole numbers).
    QPointF edge(qsizetype index, bool leading, quint32 pageIndex = 0) {
        const PageText& text = page(pageIndex);
        const QRectF r = text.chars.at(index).rect;
        const double k = leading ? 0.25 : 0.75;
        for (const PageText::Line& line : text.lines()) {
            if (index < line.first || index > line.last) {
                continue;
            }
            if (line.vertical) {
                const double y =
                    line.reversed ? r.bottom() - k * r.height() : r.top() + k * r.height();
                return {r.center().x(), y};
            }
            const double x = line.reversed ? r.right() - k * r.width() : r.left() + k * r.width();
            return {x, r.center().y()};
        }
        return r.center();
    }

    // Drags from character `from` to character `to` so that both are selected: forwards, from the
    // start of `from` to the end of `to`; backwards, from the end of `from` to the start of `to`.
    void drag(qsizetype from, qsizetype to, quint32 pageIndex = 0) {
        const bool forwards = from <= to;
        const QPointF press = edge(from, forwards, pageIndex);
        const QPointF release = edge(to, !forwards, pageIndex);
        QTest::mousePress(viewport(), Qt::LeftButton, {}, at(pageIndex, press));
        QTest::mouseMove(viewport(), at(pageIndex, page(pageIndex).chars.at(to).rect.center()));
        QTest::mouseMove(viewport(), at(pageIndex, release));
        QTest::mouseRelease(viewport(), Qt::LeftButton, {}, at(pageIndex, release));
    }

    QPoint centreOf(qsizetype index, quint32 pageIndex = 0) {
        return at(pageIndex, page(pageIndex).chars.at(index).rect.center());
    }

    // What copying puts on the clipboard.
    QString copied() {
        QGuiApplication::clipboard()->setText(QStringLiteral("(nothing)"));
        tab->copy();
        QTest::qWaitFor(
            [] { return QGuiApplication::clipboard()->text() != QStringLiteral("(nothing)"); },
            kWaitMs);
        return QGuiApplication::clipboard()->text();
    }
};

// The highlights of the selection as lines of text, one per rectangle, in points of the page.
QString describeRects(vellora::TextSelection* selection, quint32 pages) {
    QString out;
    for (quint32 page = 0; page < pages; ++page) {
        for (const QRectF& r : selection->rectsOn(page)) {
            out += QStringLiteral("page %1: %2 %3 %4 %5\n")
                       .arg(page + 1)
                       .arg(r.left(), 0, 'f', 1)
                       .arg(r.top(), 0, 'f', 1)
                       .arg(r.right(), 0, 'f', 1)
                       .arg(r.bottom(), 0, 'f', 1);
        }
    }
    return out;
}

// A golden: `[name]` followed by the lines of `describeRects`.
QString goldenFile() {
    return QStringLiteral(VELLORA_TEXT_GOLDEN);
}

QMap<QString, QString> readGolden() {
    QMap<QString, QString> sections;
    QFile file(goldenFile());
    if (!file.open(QIODevice::ReadOnly)) {
        return sections;
    }
    QString name;
    for (const QString& line : QString::fromUtf8(file.readAll()).split(QLatin1Char('\n'))) {
        if (line.startsWith(QLatin1Char('['))) {
            name = line.mid(1, line.size() - 2);
        } else if (!name.isEmpty() && !line.isEmpty()) {
            sections[name] += line + QLatin1Char('\n');
        }
    }
    return sections;
}

// Rectangle lists equal within `tolerance` points; the text of the lines must match.
bool sameRects(const QString& actual, const QString& golden, double tolerance, QString* why) {
    const QStringList a = actual.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    const QStringList g = golden.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    if (a.size() != g.size()) {
        *why = QStringLiteral("%1 rectangles, golden has %2").arg(a.size()).arg(g.size());
        return false;
    }
    for (qsizetype i = 0; i < a.size(); ++i) {
        const QStringList x = a.at(i).split(QLatin1Char(' '));
        const QStringList y = g.at(i).split(QLatin1Char(' '));
        if (x.size() != 6 || y.size() != 6 || x.at(1) != y.at(1)) {
            *why = QStringLiteral("line %1: '%2' against '%3'").arg(i).arg(a.at(i), g.at(i));
            return false;
        }
        for (int k = 2; k < 6; ++k) {
            if (std::abs(x.at(k).toDouble() - y.at(k).toDouble()) > tolerance) {
                *why = QStringLiteral("line %1: '%2' against '%3'").arg(i).arg(a.at(i), g.at(i));
                return false;
            }
        }
    }
    return true;
}

} // namespace

class TstText : public QObject {
    Q_OBJECT

private slots:
    // ---- the text of a page ----

    void theCharactersOfAPageHaveBoxesWordsAndLines() {
        Fixture f(textPdf({kThreeLines}));
        const PageText& page = f.page();
        QVERIFY(page.complete);
        QCOMPARE(static_cast<quint32>(page.chars.size()), page.total);
        const qsizetype h = f.find(QStringLiteral("Hello"));
        const qsizetype w = f.find(QStringLiteral("World"));
        const qsizetype s = f.find(QStringLiteral("Second"));
        QVERIFY(h >= 0 && w > h && s > w);
        QCOMPARE(page.chars.at(h).line, 0U);
        QCOMPARE(page.chars.at(w).word, 1U);
        QCOMPARE(page.chars.at(s).line, 1U);
        // Helvetica 20 at x = 72: the box is the glyph's, which starts a little after the origin
        // (the left side bearing of the H is 1.6 points).
        const double left = page.chars.at(h).rect.left();
        QVERIFY2(left > 72.5 && left < 75.0, qPrintable(QString::number(left)));
        QVERIFY(page.chars.at(s).rect.top() > page.chars.at(h).rect.bottom());
        QCOMPARE(page.lines().size(), 3);
    }

    void aPointIsFoundAsACharacterAndAsACaret() {
        Fixture f(textPdf({kThreeLines}));
        const PageText& page = f.page();
        const qsizetype w = f.find(QStringLiteral("World"));
        const QRectF box = page.chars.at(w).rect;
        QCOMPARE(page.charAt(box.center()), w);
        // The left half of a box puts the caret before it, the right half after it.
        QCOMPARE(page.caretAt(QPointF(box.left() + 1.0, box.center().y())),
                 static_cast<quint32>(w));
        QCOMPARE(page.caretAt(QPointF(box.right() - 1.0, box.center().y())),
                 static_cast<quint32>(w) + 1);
        // Left of the line, right of it, above everything and below everything.
        QCOMPARE(page.caretAt(QPointF(0.0, box.center().y())), 0U);
        QVERIFY(page.charAt(QPointF(2.0, 2.0)) < 0);
        QCOMPARE(page.caretAt(QPointF(2.0, 2.0)), 0U);
        QCOMPARE(page.caretAt(QPointF(600.0, 790.0)),
                 static_cast<quint32>(f.find(QStringLiteral("Third line")) + 10));
        // A word and a line around a character.
        const auto word = page.wordAround(w + 2);
        QCOMPARE(page.text(word.first, word.second), QStringLiteral("World"));
        const auto line = page.lineAround(w);
        QCOMPARE(page.text(line.first, line.second), QStringLiteral("Hello World\n"));
    }

    // ---- the pointer ----

    void theTextCursorShowsOverTextOnly() {
        Fixture f(textPdf({kThreeLines}));
        const qsizetype w = f.find(QStringLiteral("World"));
        QTest::mouseMove(f.viewport(), f.centreOf(w));
        QCOMPARE(f.viewport()->cursor().shape(), Qt::IBeamCursor);
        QTest::mouseMove(f.viewport(), f.at(0, QPointF(300.0, 500.0)));
        QCOMPARE(f.viewport()->cursor().shape(), Qt::ArrowCursor);
        QVERIFY(f.view().isOverText(f.centreOf(w)));
        QVERIFY(!f.view().isOverText(f.at(0, QPointF(300.0, 500.0))));
    }

    // ---- selecting ----

    void draggingSelectsCharactersAndCopyReadsThem() {
        Fixture f(textPdf({kThreeLines}));
        QVERIFY(f.selection()->isEmpty());
        f.drag(f.find(QStringLiteral("World")), f.find(QStringLiteral("World")) + 4);
        QVERIFY(!f.selection()->isEmpty());
        QCOMPARE(f.copied(), QStringLiteral("World"));
        // Part of a word works too.
        const qsizetype second = f.find(QStringLiteral("Second"));
        f.drag(second + 1, second + 3);
        QCOMPARE(f.copied(), QStringLiteral("eco"));
    }

    void aSelectionGoesAcrossLinesInReadingOrder() {
        Fixture f(textPdf({kThreeLines}));
        f.drag(f.find(QStringLiteral("World")), f.find(QStringLiteral("Second")) + 5);
        QCOMPARE(f.copied(), QStringLiteral("World\nSecond"));
        // Dragging backwards selects the same text.
        f.drag(f.find(QStringLiteral("Second")) + 5, f.find(QStringLiteral("World")));
        QCOMPARE(f.copied(), QStringLiteral("World\nSecond"));
    }

    void aDoubleClickSelectsAWordAndATripleClickALine() {
        Fixture f(textPdf({kThreeLines}));
        const QPoint inWorld = f.centreOf(f.find(QStringLiteral("World")) + 2);
        QTest::mouseDClick(f.viewport(), Qt::LeftButton, {}, inWorld);
        QCOMPARE(f.copied(), QStringLiteral("World"));
        // A third press close behind the double click takes the line, break included.
        QTest::mouseClick(f.viewport(), Qt::LeftButton, {}, inWorld);
        QCOMPARE(f.copied(), QStringLiteral("Hello World\n"));
        // A later click starts over.
        QTest::qWait(QApplication::doubleClickInterval() + 100);
        QTest::mouseClick(f.viewport(), Qt::LeftButton, {}, inWorld);
        QVERIFY(f.selection()->isEmpty());
    }

    void draggingFromADoubleClickGoesOnByWholeWords() {
        Fixture f(textPdf({kThreeLines}));
        const qsizetype hello = f.find(QStringLiteral("Hello"));
        const qsizetype second = f.find(QStringLiteral("Second"));
        QTest::mouseDClick(f.viewport(), Qt::LeftButton, {}, f.centreOf(hello + 1));
        // The second press of the double click is still down in a real double click; emulate the
        // drag with the selection model, which is what the mouse handler drives.
        const QRectF target = f.page().chars.at(second + 2).rect;
        f.selection()->extendTo(0, target.center());
        QCOMPARE(f.copied(), QStringLiteral("Hello World\nSecond"));
    }

    void shiftClickMovesTheOtherEnd() {
        Fixture f(textPdf({kThreeLines}));
        const qsizetype hello = f.find(QStringLiteral("Hello"));
        const QRectF first = f.page().chars.at(hello).rect;
        QTest::mouseClick(
            f.viewport(), Qt::LeftButton, {},
            f.at(0, QPointF(first.left() + 0.25 * first.width(), first.center().y())));
        QVERIFY(f.selection()->isEmpty());
        const QRectF last = f.page().chars.at(f.find(QStringLiteral("World")) + 4).rect;
        QTest::mouseClick(f.viewport(), Qt::LeftButton, Qt::ShiftModifier,
                          f.at(0, QPointF(last.right() - 0.25 * last.width(), last.center().y())));
        QCOMPARE(f.copied(), QStringLiteral("Hello World"));
        // Another shift click moves the same end again.
        const QRectF second = f.page().chars.at(f.find(QStringLiteral("Second")) + 5).rect;
        QTest::mouseClick(
            f.viewport(), Qt::LeftButton, Qt::ShiftModifier,
            f.at(0, QPointF(second.right() - 0.25 * second.width(), second.center().y())));
        QCOMPARE(f.copied(), QStringLiteral("Hello World\nSecond"));
    }

    void escapeAndAClickOnTheBackgroundClearTheSelection() {
        Fixture f(textPdf({kThreeLines}));
        f.drag(f.find(QStringLiteral("World")), f.find(QStringLiteral("World")) + 4);
        QVERIFY(!f.selection()->isEmpty());
        f.viewport()->setFocus();
        f.view().setFocus();
        QTest::keyClick(&f.view(), Qt::Key_Escape);
        QVERIFY(f.selection()->isEmpty());

        f.drag(f.find(QStringLiteral("World")), f.find(QStringLiteral("World")) + 4);
        QVERIFY(!f.selection()->isEmpty());
        QTest::mouseClick(f.viewport(), Qt::LeftButton, {}, QPoint(2, 2)); // beside the page
        QVERIFY(f.selection()->isEmpty());
    }

    // ---- select all ----

    void selectAllOnThePageTakesThePage() {
        Fixture f(textPdf({kThreeLines}));
        f.tab->selectPage();
        QCOMPARE(f.copied(), QStringLiteral("Hello World\nSecond line of text\nThird line"));
    }

    void selectAllTakesEveryPageAndJoinsThemWithABreak() {
        Fixture f(textPdf({kAlpha, kGamma}));
        f.tab->selectAll();
        QCOMPARE(f.copied(), QStringLiteral("Alpha beta\nGamma delta"));
    }

    void aSelectionAcrossPagesStartsAndEndsWhereItWasMade() {
        Fixture f(textPdf({kAlpha, kGamma}), 40);
        // Both pages fit at 40%.
        QVERIFY(QTest::qWaitFor([&] { return f.text()->isComplete(1); }, kWaitMs));
        const qsizetype beta = f.find(QStringLiteral("beta"), 0);
        const qsizetype delta = f.find(QStringLiteral("delta"), 1);
        const QRectF a = f.page(0).chars.at(beta).rect;
        const QRectF b = f.page(1).chars.at(delta + 4).rect;
        QTest::mousePress(f.viewport(), Qt::LeftButton, {},
                          f.at(0, QPointF(a.left() + 0.25 * a.width(), a.center().y())));
        QTest::mouseMove(f.viewport(), f.at(1, b.center()));
        QTest::mouseMove(f.viewport(),
                         f.at(1, QPointF(b.right() - 0.25 * b.width(), b.center().y())));
        QTest::mouseRelease(f.viewport(), Qt::LeftButton, {},
                            f.at(1, QPointF(b.right() - 0.25 * b.width(), b.center().y())));
        QCOMPARE(f.copied(), QStringLiteral("beta\nGamma delta"));
    }

    void selectAllOfALongDocumentAsksFirst() {
        QStringList pages;
        for (int i = 0; i < 1500; ++i) {
            pages << QStringLiteral("BT /F1 12 Tf 72 700 Td (Page %1) Tj ET").arg(i + 1);
        }
        Fixture f(textPdf(pages));
        QList<quint32> asked;
        f.tab->setSelectAllConfirmer([&asked](quint32 count) {
            asked.append(count);
            return false;
        });
        f.tab->selectAll();
        QCOMPARE(asked, (QList<quint32>{1500}));
        QVERIFY(f.selection()->isEmpty());

        f.tab->setSelectAllConfirmer([&asked](quint32 count) {
            asked.append(count);
            return true;
        });
        f.tab->selectAll();
        QCOMPARE(asked.size(), 2);
        QVERIFY(!f.selection()->isEmpty());
        QCOMPARE(f.selection()->start().page, 0U);
        QCOMPARE(f.selection()->end().page, 1499U);

        // A short document is not asked about.
        Fixture g(textPdf({kAlpha, kGamma}));
        g.tab->setSelectAllConfirmer([](quint32) {
            QTest::qFail("asked about a short document", __FILE__, __LINE__);
            return false;
        });
        g.tab->selectAll();
        QVERIFY(!g.selection()->isEmpty());
    }

    // ---- what is painted ----

    void theHighlightCoversTheSelectedCharactersAndNothingElse() {
        Fixture f(textPdf({kThreeLines}));
        const qsizetype w = f.find(QStringLiteral("World"));
        f.drag(w, w + 4);
        const QList<QRectF> rects = f.selection()->rectsOn(0);
        QCOMPARE(rects.size(), 1);
        QRectF expected = f.page().chars.at(w).rect;
        for (qsizetype i = 1; i < 5; ++i) {
            expected = expected.united(f.page().chars.at(w + i).rect);
        }
        QVERIFY2(std::abs(rects.first().left() - expected.left()) < 0.01 &&
                     std::abs(rects.first().right() - expected.right()) < 0.01,
                 qPrintable(describeRects(f.selection(), 1)));
        // The overlay paints that rectangle where the page is on screen.
        const QList<QRectF> painted = f.view().selectionOverlay()->highlights();
        QCOMPARE(painted.size(), 1);
        QVERIFY(painted.first().contains(f.centreOf(w + 2)));
        QVERIFY(!painted.first().contains(f.centreOf(w - 3)));
        QVERIFY(f.view().selectionOverlay()->testAttribute(Qt::WA_TransparentForMouseEvents));
    }

    void thePaintedHighlightFollowsZoom() {
        Fixture f(textPdf({kThreeLines}));
        const qsizetype w = f.find(QStringLiteral("World"));
        f.drag(w, w + 4);
        const QRectF before = f.view().selectionOverlay()->highlights().first();
        f.controller()->setZoom(2.0, QPointF(0.0, 0.0));
        f.controller()->goToPage(0);
        QTRY_VERIFY_WITH_TIMEOUT(f.view().selectionOverlay()->highlights().size() == 1, kWaitMs);
        const QRectF after = f.view().selectionOverlay()->highlights().first();
        QVERIFY2(std::abs(after.width() - 2.0 * before.width()) < 1.5,
                 qPrintable(QStringLiteral("%1 -> %2").arg(before.width()).arg(after.width())));
    }

    // ---- golden rectangles ----

    void theSelectionRectanglesMatchTheGoldenFile() {
        QMap<QString, QString> actual;
        {
            Fixture f(textPdf({kThreeLines}));
            const qsizetype w = f.find(QStringLiteral("World"));
            f.drag(w, w + 4);
            actual[QStringLiteral("one word")] = describeRects(f.selection(), 1);
            f.drag(w, f.find(QStringLiteral("Second")) + 5);
            actual[QStringLiteral("across two lines")] = describeRects(f.selection(), 1);
            f.tab->selectPage();
            actual[QStringLiteral("the page")] = describeRects(f.selection(), 1);
            QTest::mouseDClick(f.viewport(), Qt::LeftButton, {}, f.centreOf(w + 2));
            QTest::mouseClick(f.viewport(), Qt::LeftButton, {}, f.centreOf(w + 2));
            actual[QStringLiteral("a line by triple click")] = describeRects(f.selection(), 1);
        }
        {
            Fixture f(textPdf({kAlpha, kGamma}), 40);
            QVERIFY(QTest::qWaitFor([&] { return f.text()->isComplete(1); }, kWaitMs));
            f.tab->selectAll();
            actual[QStringLiteral("two pages")] = describeRects(f.selection(), 2);
        }
        {
            Fixture f(textPdf({kThreeLines}, 90));
            f.tab->selectPage();
            actual[QStringLiteral("turned a quarter")] = describeRects(f.selection(), 1);
        }

        if (qEnvironmentVariableIsSet("VELLORA_UPDATE_GOLDEN")) {
            QFile out(goldenFile());
            QVERIFY(out.open(QIODevice::WriteOnly | QIODevice::Truncate));
            for (auto it = actual.cbegin(); it != actual.cend(); ++it) {
                out.write(QStringLiteral("[%1]\n%2\n").arg(it.key(), it.value()).toUtf8());
            }
            return;
        }
        const QMap<QString, QString> golden = readGolden();
        QVERIFY2(!golden.isEmpty(), qPrintable(goldenFile() + " is missing or empty"));
        QCOMPARE(golden.keys(), actual.keys());
        for (auto it = actual.cbegin(); it != actual.cend(); ++it) {
            QString why;
            QVERIFY2(
                sameRects(it.value(), golden.value(it.key()), kGoldenTolerance, &why),
                qPrintable(QStringLiteral("%1: %2\nactual:\n%3").arg(it.key(), why, it.value())));
        }
    }

    // ---- a turned page ----

    void selectionAndTheCursorWorkOnATurnedPage() {
        Fixture f(textPdf({kThreeLines}, 90));
        const qsizetype w = f.find(QStringLiteral("World"));
        QTest::mouseMove(f.viewport(), f.centreOf(w));
        QCOMPARE(f.viewport()->cursor().shape(), Qt::IBeamCursor);
        f.drag(w, w + 4);
        QCOMPARE(f.copied(), QStringLiteral("World"));
        // The page is 792 wide and 612 high when turned a quarter: the text runs down it.
        const QRectF rect = f.page().chars.at(w).rect;
        QVERIFY(rect.right() <= 792.5 && rect.bottom() <= 612.5);
        QVERIFY(f.page().chars.at(w + 1).rect.top() > rect.top());
    }
};

VELLORA_TEST_MAIN(TstText)

#include "tst_text.moc"
