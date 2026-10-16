// M1 task 17a: what a screen reader sees of the canvas. The tests walk the accessibility tree of a
// real document through Qt's own API (the one the platform bridges use): the canvas is a
// document, its children are the pages, a page reads its text and has links for children.
#include "MainWindow.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"

#include <QAccessible>
#include <QAccessibleActionInterface>
#include <QAccessibleTextInterface>
#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

using vellora::CanvasView;
using vellora::DocumentTab;

namespace {

constexpr int kWaitMs = 60'000;

// Three pages of 612 x 792 points with Helvetica text. The first has two links: one to the third
// page, one to an address.
QByteArray accessiblePdf() {
    const QStringList texts = {
        QStringLiteral(
            "BT /F1 24 Tf 72 700 Td (Hello accessible world) Tj 0 -40 Td (Second line) Tj ET"),
        QStringLiteral("BT /F1 24 Tf 72 700 Td (Page two) Tj ET"),
        QStringLiteral("BT /F1 24 Tf 72 700 Td (The last page) Tj ET"),
    };
    QStringList objects;
    objects << QStringLiteral("<< /Type /Catalog /Pages 2 0 R >>");
    objects << QStringLiteral("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>");
    // 3..5 pages, 6..8 contents, 9 font.
    const QString links = QStringLiteral(
        "/Annots [<< /Type /Annot /Subtype /Link /Rect [72 500 272 540] "
        "/Dest [5 0 R /Fit] >> << /Type /Annot /Subtype /Link /Rect [72 400 272 440] "
        "/A << /S /URI /URI (https://example.org/) >> >>]");
    for (int i = 0; i < 3; ++i) {
        objects << QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents "
                                  "%1 0 R /Resources << /Font << /F1 9 0 R >> >> %2 >>")
                       .arg(6 + i)
                       .arg(i == 0 ? links : QString());
    }
    for (const QString& text : texts) {
        objects
            << QStringLiteral("<< /Length %1 >>\nstream\n%2\nendstream").arg(text.size()).arg(text);
    }
    objects << QStringLiteral(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    QByteArray out = "%PDF-1.4\n";
    QList<qsizetype> offsets;
    for (int i = 0; i < objects.size(); ++i) {
        offsets << out.size();
        out += QStringLiteral("%1 0 obj\n%2\nendobj\n").arg(i + 1).arg(objects.at(i)).toUtf8();
    }
    const qsizetype xref = out.size();
    out += QStringLiteral("xref\n0 %1\n0000000000 65535 f \n").arg(objects.size() + 1).toUtf8();
    for (const qsizetype offset : offsets) {
        out += QStringLiteral("%1 00000 n \n").arg(offset, 10, 10, QLatin1Char('0')).toUtf8();
    }
    out += QStringLiteral("trailer\n<< /Size %1 /Root 1 0 R >>\nstartxref\n%2\n%%EOF\n")
               .arg(objects.size() + 1)
               .arg(xref)
               .toUtf8();
    return out;
}

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    DocumentTab* tab = nullptr;

    Fixture() {
        QFile file(dir.filePath(QStringLiteral("access.pdf")));
        if (!file.open(QIODevice::WriteOnly)) {
            qFatal("cannot write the test document");
        }
        file.write(accessiblePdf());
        file.close();
        window.resize(900, 900);
        window.show();
        tab = &window.currentTab();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(dir.filePath(QStringLiteral("access.pdf"))) ||
            !opened.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
        controller()->setZoom(0.6, QPointF(0.0, 0.0));
        controller()->goToPage(0);
    }

    CanvasView& view() { return window.canvas(); }
    vellora::CanvasController* controller() { return view().controller(); }
    QAccessibleInterface* canvas() { return QAccessible::queryAccessibleInterface(&view()); }

    // The text of `page` once it has arrived (empty if it never does).
    QString textOf(int page) {
        QAccessibleTextInterface* text = canvas()->child(page)->textInterface();
        if (!QTest::qWaitFor([&] { return text->characterCount() > 0; }, kWaitMs)) {
            return {};
        }
        return text->text(0, text->characterCount());
    }
};

QList<QString> g_announcements;
void collect(QAccessibleEvent* event) {
    if (event->type() == QAccessible::Announcement) {
        g_announcements.append(static_cast<QAccessibleAnnouncementEvent*>(event)->message());
    }
}

} // namespace

class TstAccessibility : public QObject {
    Q_OBJECT

private slots:
    void theCanvasIsADocumentWithAPageForEachPage() {
        Fixture f;
        QAccessibleInterface* canvas = f.canvas();
        QVERIFY(canvas != nullptr);
        QCOMPARE(canvas->role(), QAccessible::Document);
        QCOMPARE(canvas->text(QAccessible::Name), QStringLiteral("access.pdf"));
        QVERIFY(canvas->state().readOnly);
        QCOMPARE(canvas->childCount(), 3);
        for (int i = 0; i < canvas->childCount(); ++i) {
            QAccessibleInterface* page = canvas->child(i);
            QVERIFY(page != nullptr);
            QVERIFY(page->isValid());
            QCOMPARE(page->role(), QAccessible::Section);
            QCOMPARE(page->parent(), canvas);
            QCOMPARE(canvas->indexOfChild(page), i);
            QCOMPARE(page->text(QAccessible::Name), QStringLiteral("Page %1 of 3").arg(i + 1));
            // The same interface every time.
            QCOMPARE(canvas->child(i), page);
        }
        QVERIFY(canvas->child(3) == nullptr);
        QVERIFY(canvas->child(-1) == nullptr);
    }

    void pagesReadTheirTextInReadingOrder() {
        Fixture f;
        const QString first = f.textOf(0);
        QVERIFY(!first.isEmpty());
        QVERIFY2(first.contains(QStringLiteral("Hello accessible world")), qPrintable(first));
        QVERIFY2(first.indexOf(QStringLiteral("Hello")) <
                     first.indexOf(QStringLiteral("Second line")),
                 qPrintable(first));
        // A page that is not on screen is read too (its text is asked for when a reader looks).
        QCOMPARE(f.textOf(2).trimmed(), QStringLiteral("The last page"));
        QCOMPARE(f.textOf(1).trimmed(), QStringLiteral("Page two"));

        QAccessibleTextInterface* text = f.canvas()->child(0)->textInterface();
        QCOMPARE(text->text(6, 16), QStringLiteral("accessible"));
        QCOMPARE(text->text(0, 5), QStringLiteral("Hello"));
        // Out of range reads are cut, not errors.
        QCOMPARE(text->text(-4, 5), QStringLiteral("Hello"));
        QCOMPARE(text->text(text->characterCount() - 2, 9999).size(), 2);
    }

    void charactersHaveRectanglesAndPointsFindTheirCharacter() {
        Fixture f;
        QVERIFY(!f.textOf(0).isEmpty());
        QAccessibleInterface* page = f.canvas()->child(0);
        QAccessibleTextInterface* text = page->textInterface();
        const QRect pageRect = page->rect();
        QVERIFY(!pageRect.isEmpty());
        QRect previous;
        for (const int offset : {0, 1, 6, 12}) {
            const QRect box = text->characterRect(offset);
            QVERIFY2(!box.isEmpty(), qPrintable(QString::number(offset)));
            QVERIFY(pageRect.contains(box));
            // The same line reads left to right.
            if (!previous.isNull()) {
                QVERIFY(box.left() > previous.left());
            }
            previous = box;
            // A point in the left half of a character is the gap before it.
            QCOMPARE(text->offsetAtPoint(QPoint(box.left() + box.width() / 4, box.center().y())),
                     offset);
        }
        // Nothing to find outside the page.
        QCOMPARE(text->offsetAtPoint(QPoint(-10000, -10000)), -1);
    }

    void aPageKnowsWhetherItIsOnScreen() {
        Fixture f;
        QAccessibleInterface* canvas = f.canvas();
        QVERIFY(!canvas->child(0)->state().offscreen);
        QVERIFY(canvas->child(2)->state().offscreen);
        f.controller()->goToPage(2);
        QTRY_VERIFY_WITH_TIMEOUT(!canvas->child(2)->state().offscreen, kWaitMs);
        QVERIFY(canvas->child(0)->state().offscreen);
        // Hit testing finds the page under a point.
        const QRect box = canvas->child(2)->rect();
        QVERIFY(box.isValid());
        QCOMPARE(canvas->childAt(box.center().x(), box.center().y()), canvas->child(2));
    }

    void theSelectionIsReportedAsARangeOfTheText() {
        Fixture f;
        const QString text = f.textOf(0);
        QVERIFY(!text.isEmpty());
        QAccessibleTextInterface* page = f.canvas()->child(0)->textInterface();
        QCOMPARE(page->selectionCount(), 0);
        f.view().selection()->selectPage(0);
        QCOMPARE(page->selectionCount(), 1);
        int start = -1;
        int end = -1;
        page->selection(0, &start, &end);
        QCOMPARE(start, 0);
        QCOMPARE(end, static_cast<int>(text.size()));
    }

    void linksAreFocusableChildrenThatCanBeFollowed() {
        Fixture f;
        QAccessibleInterface* page = f.canvas()->child(0);
        QTRY_COMPARE_WITH_TIMEOUT(page->childCount(), 2, kWaitMs);
        QAccessibleInterface* goTo = page->child(0);
        QAccessibleInterface* uri = page->child(1);
        QVERIFY(goTo != nullptr && uri != nullptr);
        QCOMPARE(goTo->role(), QAccessible::Link);
        QCOMPARE(goTo->parent(), page);
        QCOMPARE(page->indexOfChild(uri), 1);
        QVERIFY(goTo->state().focusable);
        QVERIFY(goTo->state().linked);
        QVERIFY(!goTo->state().focused);
        QVERIFY2(goTo->text(QAccessible::Name).contains(QStringLiteral("3")),
                 qPrintable(goTo->text(QAccessible::Name)));
        QVERIFY2(uri->text(QAccessible::Name).contains(QStringLiteral("example.org")),
                 qPrintable(uri->text(QAccessible::Name)));
        QVERIFY(page->rect().contains(goTo->rect()));
        QCOMPARE(page->childAt(goTo->rect().center().x(), goTo->rect().center().y()), goTo);
        QVERIFY(page->child(2) == nullptr);

        QAccessibleActionInterface* actions = goTo->actionInterface();
        QVERIFY(actions != nullptr);
        QVERIFY(actions->actionNames().contains(QAccessibleActionInterface::pressAction()));
        QVERIFY(actions->actionNames().contains(QAccessibleActionInterface::setFocusAction()));

        // Focusing is the screen reader's cursor: one link at a time, and the canvas says which.
        QVERIFY(!f.view().focusedLink());
        QVERIFY(f.canvas()->focusChild() == nullptr);
        actions->doAction(QAccessibleActionInterface::setFocusAction());
        QVERIFY(f.view().focusedLink());
        QCOMPARE(f.view().focusedLink()->index, 0);
        QVERIFY(goTo->state().focused);
        QVERIFY(!uri->state().focused);
        QCOMPARE(f.canvas()->focusChild(), goTo);

        // Pressing follows it, like a click (the owner decides what that does).
        QSignalSpy followed(&f.view(), &CanvasView::linkActivated);
        actions->doAction(QAccessibleActionInterface::pressAction());
        QCOMPARE(followed.size(), 1);
        QCOMPARE(followed.first().first().value<vellora::Link>().kind, vellora::LinkKind::GoTo);

        // Enter does the same from the keyboard while a link is focused; Escape lets go.
        QTest::keyClick(&f.view(), Qt::Key_Return);
        QCOMPARE(followed.size(), 2);
        QTest::keyClick(&f.view(), Qt::Key_Escape);
        QVERIFY(!f.view().focusedLink());
        QVERIFY(!goTo->state().focused);
        QTest::keyClick(&f.view(), Qt::Key_Return);
        QCOMPARE(followed.size(), 2);

        // A link that is not there cannot be focused.
        QVERIFY(!f.view().focusLink(0, 7));
        QVERIFY(!f.view().focusLink(2, 0));
    }

    void focusingALinkBringsItIntoView() {
        Fixture f;
        QAccessibleInterface* page = f.canvas()->child(0);
        QTRY_COMPARE_WITH_TIMEOUT(page->childCount(), 2, kWaitMs);
        // Zoomed in and scrolled away from the links.
        f.controller()->setZoom(2.0, QPointF(0.0, 0.0));
        f.controller()->scrollBy(QPointF(0.0, 3000.0));
        QAccessibleInterface* link = page->child(1);
        QVERIFY(link->state().offscreen);
        QVERIFY(f.view().focusLink(0, 1));
        QTRY_VERIFY_WITH_TIMEOUT(!link->state().offscreen, kWaitMs);
    }

    void aPageChangeIsAnnouncedToAScreenReader() {
        Fixture f;
        QAccessible::setActive(true);
        g_announcements.clear();
        const auto previous = QAccessible::installUpdateHandler(&collect);
        f.controller()->goToPage(1);
        QTRY_VERIFY_WITH_TIMEOUT(!g_announcements.isEmpty(), kWaitMs);
        QCOMPARE(g_announcements.last(), QStringLiteral("Page 2 of 3"));
        f.controller()->goToPage(2);
        QTRY_COMPARE_WITH_TIMEOUT(g_announcements.last(), QStringLiteral("Page 3 of 3"), kWaitMs);
        QAccessible::installUpdateHandler(previous);
        QAccessible::setActive(false);
    }

    void pagesAreNamedWithTheDocumentsOwnLabels() {
        Fixture f;
        // The tab names a page by its label (here the page number, as the file defines none).
        QCOMPARE(f.canvas()->child(1)->text(QAccessible::Name), QStringLiteral("Page 2 of 3"));
        QCOMPARE(f.canvas()->text(QAccessible::Description), QStringLiteral("Page 1 of 3"));
    }

    void anotherDocumentReplacesTheTree() {
        Fixture f;
        QAccessibleInterface* canvas = f.canvas();
        QAccessibleInterface* oldPage = canvas->child(2);
        QVERIFY(oldPage != nullptr);
        QCOMPARE(canvas->childCount(), 3);

        QTemporaryDir other;
        QFile file(other.filePath(QStringLiteral("one.pdf")));
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(
            "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type "
            "/Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R "
            "/MediaBox [0 0 200 100] >>\nendobj\ntrailer\n<< /Size 4 /Root 1 0 R >>\n%%EOF\n");
        file.close();
        QSignalSpy opened(&f.window.session(), &vellora::EngineSession::opened);
        QVERIFY(f.tab->open(other.filePath(QStringLiteral("one.pdf"))));
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(canvas->text(QAccessible::Name), QStringLiteral("one.pdf"));
        QTRY_COMPARE_WITH_TIMEOUT(canvas->childCount(), 1, kWaitMs);
        QVERIFY(canvas->child(1) == nullptr);
        QCOMPARE(canvas->child(0)->text(QAccessible::Name), QStringLiteral("Page 1 of 1"));
    }
};

VELLORA_TEST_MAIN(TstAccessibility)
#include "tst_accessibility.moc"
