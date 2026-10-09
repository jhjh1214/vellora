// M1 task 12c: the outline in the sidebar, page labels, Go to Page and the jump history, with a
// real engine and no GPU. The documents are small synthetic files: what is tested is the UI's
// behaviour (lazy levels, following the page, jumping, labels), not PDF reading, which the engine
// tests cover.
#include "MainWindow.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"
#include "navigation/NavigationHistory.h"
#include "settings/AppSettings.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QFile>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QToolTip>

using vellora::CanvasController;
using vellora::NavigationHistory;
using vellora::OutlineView;

namespace {

constexpr int kWaitMs = 60'000;
constexpr int kPages = 8;
// Page `index` is object `3 + index`; the outline objects follow the pages.
constexpr int kFirstOutlineObject = 3 + kPages;

// A document of `kPages` pages of 612 x 792 points with `objects` after the pages (numbered from
// `kFirstOutlineObject`) and `catalogExtra` in the catalog.
QByteArray buildPdf(const QString& catalogExtra, const QStringList& objects) {
    QStringList kids;
    for (int i = 0; i < kPages; ++i) {
        kids << QStringLiteral("%1 0 R").arg(3 + i);
    }
    QStringList all;
    all << QStringLiteral("<< /Type /Catalog /Pages 2 0 R %1 >>").arg(catalogExtra);
    all << QStringLiteral("<< /Type /Pages /Kids [%1] /Count %2 >>")
               .arg(kids.join(QLatin1Char(' ')))
               .arg(kPages);
    for (int i = 0; i < kPages; ++i) {
        all << QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>");
    }
    all << objects;
    QByteArray out = "%PDF-1.4\n";
    QList<qsizetype> offsets;
    for (int i = 0; i < all.size(); ++i) {
        offsets << out.size();
        out += QStringLiteral("%1 0 obj\n%2\nendobj\n").arg(i + 1).arg(all.at(i)).toUtf8();
    }
    const qsizetype xref = out.size();
    out += QStringLiteral("xref\n0 %1\n0000000000 65535 f \n").arg(all.size() + 1).toUtf8();
    for (const qsizetype offset : offsets) {
        out += QStringLiteral("%1 00000 n \n").arg(offset, 10, 10, QLatin1Char('0')).toUtf8();
    }
    out += QStringLiteral("trailer\n<< /Size %1 /Root 1 0 R >>\nstartxref\n%2\n%%EOF\n")
               .arg(all.size() + 1)
               .arg(xref)
               .toUtf8();
    return out;
}

QString at(int page, const QString& rest) {
    return QStringLiteral("[%1 0 R %2]").arg(3 + page).arg(rest);
}

// Object numbers of the outline of `documented()`.
constexpr int kRoot = kFirstOutlineObject;
constexpr int kPreface = kRoot + 1;
constexpr int kChapter = kRoot + 2;
constexpr int kSection1 = kRoot + 3;
constexpr int kSection2 = kRoot + 4;
constexpr int kPart = kRoot + 5;
constexpr int kPartIntro = kRoot + 6;
constexpr int kPartDetail = kRoot + 7;
constexpr int kAppendix = kRoot + 8;

// Pages labelled i, ii, 1, 2, 3, A-1, A-2, A-3, with this outline (page, destination):
//   Preface (1, Fit)
//   Chapter 1 (3, XYZ with a point mid-page; open)
//     Section 1.1 (3, XYZ at the top)      Section 1.2 (5, FitH)
//   Part 2 (6, Fit; closed)
//     Part 2 intro (6)      Part 2 detail (7)
//   <b>Appendix</b> & more (8, bold)
QByteArray documented() {
    const QString catalog =
        QStringLiteral(
            "/Outlines %1 0 R "
            "/PageLabels << /Nums [0 << /S /r >> 2 << /S /D >> 5 << /S /D /P (A-) >>] >>")
            .arg(kRoot);
    const QStringList objects = {
        QStringLiteral("<< /Type /Outlines /First %1 0 R /Last %2 0 R /Count 4 >>")
            .arg(kPreface)
            .arg(kAppendix),
        QStringLiteral("<< /Title (Preface) /Parent %1 0 R /Next %2 0 R /Dest %3 >>")
            .arg(kRoot)
            .arg(kChapter)
            .arg(at(0, QStringLiteral("/Fit"))),
        QStringLiteral("<< /Title (Chapter 1) /Parent %1 0 R /Prev %2 0 R /Next %3 0 R "
                       "/First %4 0 R /Last %5 0 R /Count 2 /Dest %6 >>")
            .arg(kRoot)
            .arg(kPreface)
            .arg(kPart)
            .arg(kSection1)
            .arg(kSection2)
            .arg(at(2, QStringLiteral("/XYZ 0 400 null"))),
        QStringLiteral("<< /Title (Section 1.1) /Parent %1 0 R /Next %2 0 R /Dest %3 >>")
            .arg(kChapter)
            .arg(kSection2)
            .arg(at(2, QStringLiteral("/XYZ 0 792 null"))),
        QStringLiteral("<< /Title (Section 1.2) /Parent %1 0 R /Prev %2 0 R /Dest %3 >>")
            .arg(kChapter)
            .arg(kSection1)
            .arg(at(4, QStringLiteral("/FitH 600"))),
        QStringLiteral("<< /Title (Part 2) /Parent %1 0 R /Prev %2 0 R /Next %3 0 R "
                       "/First %4 0 R /Last %5 0 R /Count -2 /Dest %6 >>")
            .arg(kRoot)
            .arg(kChapter)
            .arg(kAppendix)
            .arg(kPartIntro)
            .arg(kPartDetail)
            .arg(at(5, QStringLiteral("/Fit"))),
        QStringLiteral("<< /Title (Part 2 intro) /Parent %1 0 R /Next %2 0 R /Dest %3 >>")
            .arg(kPart)
            .arg(kPartDetail)
            .arg(at(5, QStringLiteral("/Fit"))),
        QStringLiteral("<< /Title (Part 2 detail) /Parent %1 0 R /Prev %2 0 R /Dest %3 >>")
            .arg(kPart)
            .arg(kPartIntro)
            .arg(at(6, QStringLiteral("/Fit"))),
        QStringLiteral("<< /Title (<b>Appendix</b> & more) /Parent %1 0 R /Prev %2 0 R /F 3 "
                       "/Dest %3 >>")
            .arg(kRoot)
            .arg(kPart)
            .arg(at(7, QStringLiteral("/Fit"))),
    };
    return buildPdf(catalog, objects);
}

// An outline of `count` top-level items, all going to page 1.
QByteArray longOutline(int count) {
    QStringList objects;
    objects << QStringLiteral("<< /Type /Outlines /First %1 0 R /Last %2 0 R /Count %3 >>")
                   .arg(kRoot + 1)
                   .arg(kRoot + count)
                   .arg(count);
    for (int n = 1; n <= count; ++n) {
        QString item = QStringLiteral("<< /Title (Item %1) /Parent %2 0 R /Dest %3")
                           .arg(n)
                           .arg(kRoot)
                           .arg(at(0, QStringLiteral("/Fit")));
        if (n > 1) {
            item += QStringLiteral(" /Prev %1 0 R").arg(kRoot + n - 1);
        }
        if (n < count) {
            item += QStringLiteral(" /Next %1 0 R").arg(kRoot + n + 1);
        }
        objects << item + QStringLiteral(" >>");
    }
    return buildPdf(QStringLiteral("/Outlines %1 0 R").arg(kRoot), objects);
}

QString writePdf(const QTemporaryDir& dir, const QString& name, const QByteArray& bytes) {
    const QString path = dir.filePath(name);
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly)) {
        qFatal("cannot write %s", qPrintable(path));
    }
    file.write(bytes);
    return path;
}

// A window with `path` open, its sidebar on the outline tab.
struct Fixture {
    vellora::MainWindow window;
    vellora::DocumentTab* tab = nullptr;

    explicit Fixture(const QString& path) {
        window.resize(900, 700);
        window.show();
        tab = &window.currentTab();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(path) || !opened.wait(kWaitMs)) {
            qFatal("could not open %s", qPrintable(path));
        }
        tab->showOutline();
    }

    OutlineView& outline() { return tab->sidebar().outline(); }
    CanvasController* controller() { return window.canvas().controller(); }
    QPoint centreOf(quint32 id) {
        return outline().visualItemRect(outline().itemForId(id)).center();
    }
};

} // namespace

class TstNavigation : public QObject {
    Q_OBJECT

private slots:
    // ---- the history ----

    void backAndForwardReturnToThePlacesJumpedFrom() {
        NavigationHistory history;
        QVERIFY(!history.canGoBack());
        QVERIFY(!history.canGoForward());
        QVERIFY(!history.back({9, 1.0, 1.0}));
        QVERIFY(!history.forward({9, 1.0, 1.0}));

        history.recordJump({1, 10.0, 1.0});
        history.recordJump({5, 20.0, 2.0});
        QCOMPARE(history.backCount(), 2);
        // Back from page 9 returns to the last place left, and Forward to where Back left from.
        QCOMPARE(history.back({9, 0.0, 1.5}), (NavigationHistory::Place{5, 20.0, 2.0}));
        QCOMPARE(history.back({5, 20.0, 2.0}), (NavigationHistory::Place{1, 10.0, 1.0}));
        QVERIFY(!history.canGoBack());
        QCOMPARE(history.forward({1, 10.0, 1.0}), (NavigationHistory::Place{5, 20.0, 2.0}));
        QCOMPARE(history.forward({5, 20.0, 2.0}), (NavigationHistory::Place{9, 0.0, 1.5}));
        QVERIFY(!history.canGoForward());
    }

    void aNewJumpAfterGoingBackForgetsWhatWasAhead() {
        NavigationHistory history;
        history.recordJump({1, 0.0, 1.0});
        QVERIFY(history.back({4, 0.0, 1.0}));
        QVERIFY(history.canGoForward());
        QSignalSpy changed(&history, &NavigationHistory::changed);
        history.recordJump({1, 0.0, 1.0});
        QVERIFY(!history.canGoForward());
        QCOMPARE(changed.size(), 1);
    }

    void theHistoryIsBoundedAndSkipsRepeats() {
        NavigationHistory history;
        for (quint32 page = 0; page < NavigationHistory::kMaxEntries + 20; ++page) {
            history.recordJump({page, 0.0, 1.0});
        }
        QCOMPARE(history.backCount(), NavigationHistory::kMaxEntries);
        // The oldest were dropped; the newest is the first to come back.
        QCOMPARE(history.back({0, 0.0, 1.0})->page, NavigationHistory::kMaxEntries + 19U);

        NavigationHistory repeats;
        repeats.recordJump({3, 0.0, 1.0});
        repeats.recordJump({3, 0.0, 1.0});
        QCOMPARE(repeats.backCount(), 1);
        repeats.clear();
        QVERIFY(!repeats.canGoBack());
    }

    // ---- the outline tree ----

    void theTopLevelIsReadWhenTheDocumentOpensAndOpenItemsAreExpanded() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.topLevelLoaded(), kWaitMs);
        QCOMPARE(tree.topLevelItemCount(), 4);
        QCOMPARE(tree.topLevelItem(0)->text(0), QStringLiteral("Preface"));
        QCOMPARE(tree.topLevelItem(1)->text(0), QStringLiteral("Chapter 1"));
        QCOMPARE(tree.topLevelItem(2)->text(0), QStringLiteral("Part 2"));
        // Chapter 1 asks to be open: its children are read and shown without a click.
        QTreeWidgetItem* chapter = tree.itemForId(kChapter);
        QVERIFY(chapter->isExpanded());
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kSection2) != nullptr, kWaitMs);
        QCOMPARE(chapter->childCount(), 2);
        QCOMPARE(chapter->child(0)->text(0), QStringLiteral("Section 1.1"));
        QVERIFY(tree.destinationOf(chapter).has_value());
        QCOMPARE(tree.destinationOf(chapter)->page, 2U);
    }

    void theChildrenOfAClosedItemAreOnlyReadWhenItIsExpanded() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kSection2) != nullptr, kWaitMs);
        QTRY_COMPARE_WITH_TIMEOUT(tree.requestsInFlight(), 0, kWaitMs);
        QTreeWidgetItem* part = tree.itemForId(kPart);
        QVERIFY(!part->isExpanded());
        // A line stands in for the children, and nothing was asked for.
        QCOMPARE(part->childCount(), 1);
        QVERIFY(tree.itemForId(kPartIntro) == nullptr);

        tree.expandItem(part);
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kPartDetail) != nullptr, kWaitMs);
        QCOMPARE(part->childCount(), 2);
        QCOMPARE(part->child(1)->text(0), QStringLiteral("Part 2 detail"));
    }

    void aLevelLongerThanOneRequestIsReadPageByPage() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("long.pdf"), longOutline(300)));
        QSignalSpy answers(&f.window.session(), &vellora::EngineSession::outlineReady);
        OutlineView& tree = f.outline();
        QTRY_COMPARE_WITH_TIMEOUT(tree.topLevelItemCount(), 300, kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(tree.topLevelLoaded(), kWaitMs);
        QCOMPARE(tree.topLevelItem(299)->text(0), QStringLiteral("Item 300"));
        QCOMPARE(tree.loadedItems(), 300);
        // 128 + 128 + 44: three requests, though the first one may have been answered before the
        // spy existed.
        QVERIFY(answers.size() <= 3);
    }

    void titlesAreShownAsPlainTextAndTheToolTipIsEscaped() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kAppendix) != nullptr, kWaitMs);
        QTreeWidgetItem* appendix = tree.itemForId(kAppendix);
        QCOMPARE(appendix->text(0), QStringLiteral("<b>Appendix</b> & more"));
        const QString tip = appendix->toolTip(0);
        QVERIFY2(!tip.contains(QStringLiteral("<b>")), qPrintable(tip));
        QVERIFY2(tip.contains(QStringLiteral("&lt;b&gt;Appendix&lt;/b&gt; &amp; more")),
                 qPrintable(tip));
        QVERIFY(appendix->font(0).bold());
        QVERIFY(appendix->font(0).italic());
        QVERIFY(!tree.itemForId(kPreface)->font(0).bold());
    }

    void aDocumentWithoutAnOutlineSaysSo() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.topLevelLoaded(), kWaitMs);
        QVERIFY(tree.isEmptyOutline());
        QCOMPARE(tree.loadedItems(), 0);
    }

    // ---- jumping ----

    void clickingAnItemJumpsToItsDestinationAndRecordsTheJump() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.outline().itemForId(kSection2) != nullptr, kWaitMs);
        QCOMPARE(f.controller()->currentPage(), 0U);
        QCOMPARE(f.tab->history().backCount(), 0);

        // Section 1.2: page 5, "fit width".
        QTest::mouseClick(f.outline().viewport(), Qt::LeftButton, {}, f.centreOf(kSection2));
        QCOMPARE(f.controller()->currentPage(), 4U);
        QCOMPARE(f.controller()->zoomMode(), CanvasController::ZoomMode::FitWidth);
        QCOMPARE(f.tab->history().backCount(), 1);
    }

    void aPointDestinationPutsThatPointAtTheTopOfTheWindow() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.outline().itemForId(kChapter) != nullptr, kWaitMs);
        // Chapter 1: page 3, the point 400 points above the bottom of a 792 point page, so 392
        // points below its top.
        QTest::mouseClick(f.outline().viewport(), Qt::LeftButton, {}, f.centreOf(kChapter));
        const auto top = f.controller()->topAnchor();
        QCOMPARE(top.page, 2U);
        QVERIFY2(qAbs(top.offsetPoints - 392.0) < 1.0,
                 qPrintable(QString::number(top.offsetPoints)));
    }

    void backAndForwardReturnToTheExactPlace() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.outline().itemForId(kSection2) != nullptr, kWaitMs);
        // Scroll a little, so that "where the reader was" is not a page start.
        f.controller()->scrollBy({0.0, 123.0});
        const NavigationHistory::Place before = f.tab->currentPlace();
        QCOMPARE(f.tab->history().backCount(), 0); // scrolling is not a jump

        QTest::mouseClick(f.outline().viewport(), Qt::LeftButton, {}, f.centreOf(kSection2));
        const NavigationHistory::Place after = f.tab->currentPlace();
        QVERIFY(after.page != before.page);

        QVERIFY(f.tab->goBack());
        const NavigationHistory::Place back = f.tab->currentPlace();
        QCOMPARE(back.page, before.page);
        QVERIFY(qAbs(back.offsetPoints - before.offsetPoints) < 0.5);
        QVERIFY(qAbs(back.zoom - before.zoom) < 1e-6);
        QVERIFY(!f.tab->goBack());

        QVERIFY(f.tab->goForward());
        QCOMPARE(f.tab->currentPlace().page, after.page);
        QVERIFY(!f.tab->goForward());
    }

    void theCommandsFollowTheHistory() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.outline().itemForId(kSection2) != nullptr, kWaitMs);
        const auto shortcut = [&](const QString& id) {
            const vellora::Command* command = f.window.commands().find(id);
            return command != nullptr && !command->shortcuts.isEmpty() ? command->shortcuts.first()
                                                                       : QKeySequence();
        };
        QCOMPARE(shortcut(QStringLiteral("view.back")), QKeySequence(Qt::ALT | Qt::Key_Left));
        QCOMPARE(shortcut(QStringLiteral("view.forward")), QKeySequence(Qt::ALT | Qt::Key_Right));
        QCOMPARE(shortcut(QStringLiteral("view.goToPage")), QKeySequence(Qt::CTRL | Qt::Key_G));
        QVERIFY(f.window.commands().find(QStringLiteral("view.outline")) != nullptr);

        QTest::mouseClick(f.outline().viewport(), Qt::LeftButton, {}, f.centreOf(kSection2));
        QVERIFY(f.window.commands().run(QStringLiteral("view.back")));
        QCOMPARE(f.controller()->currentPage(), 0U);
        QVERIFY(f.window.commands().run(QStringLiteral("view.forward")));
        QCOMPARE(f.controller()->currentPage(), 4U);
    }

    // ---- following the page ----

    void theTreeExpandsToTheSectionOfTheCurrentPageAndSelectsIt() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kSection2) != nullptr, kWaitMs);
        QVERIFY(!tree.itemForId(kPart)->isExpanded());

        // Page 7 is in "Part 2 detail", inside the closed "Part 2": the tree opens it.
        f.controller()->goToPage(6);
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kPartDetail) != nullptr, kWaitMs);
        QTRY_COMPARE_WITH_TIMEOUT(tree.currentItem(), tree.itemForId(kPartDetail), kWaitMs);
        QVERIFY(tree.itemForId(kPart)->isExpanded());
        // Following is not a jump.
        QCOMPARE(f.tab->history().backCount(), 0);
        QCOMPARE(f.controller()->currentPage(), 6U);

        // A page before every section selects nothing new; a page in the first chapter selects
        // the deepest section there.
        f.controller()->goToPage(3);
        QTRY_COMPARE_WITH_TIMEOUT(tree.currentItem(), tree.itemForId(kSection1), kWaitMs);
    }

    void followingDoesNotMoveTheSelectionOfAnItemJustClicked() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        OutlineView& tree = f.outline();
        QTRY_VERIFY_WITH_TIMEOUT(tree.itemForId(kSection2) != nullptr, kWaitMs);
        // Chapter 1 and Section 1.1 both start on page 3: after clicking the chapter, the tree
        // would pick the section (the deepest) if it followed blindly.
        QTest::mouseClick(tree.viewport(), Qt::LeftButton, {}, f.centreOf(kChapter));
        QCOMPARE(tree.currentItem(), tree.itemForId(kChapter));
        QTest::qWait(OutlineView::kFollowDelayMs * 3);
        QTRY_COMPARE_WITH_TIMEOUT(tree.requestsInFlight(), 0, kWaitMs);
        QCOMPARE(tree.currentItem(), tree.itemForId(kChapter));
    }

    // ---- page labels ----

    void labelsReachTheThumbnailsAndTheStatusText() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->hasPageLabels(), kWaitMs);
        QCOMPARE(f.tab->pageLabel(0), QStringLiteral("i"));
        QCOMPARE(f.tab->pageLabel(2), QStringLiteral("1"));
        QCOMPARE(f.tab->pageLabel(7), QStringLiteral("A-3"));
        QCOMPARE(f.tab->sidebar().thumbnails().labelFor(5), QStringLiteral("A-1"));

        f.controller()->goToPage(1);
        QTRY_COMPARE_WITH_TIMEOUT(f.tab->pageStatus(), QStringLiteral("Page ii (2 / 8)"), kWaitMs);
        f.controller()->goToPage(2);
        QTRY_COMPARE_WITH_TIMEOUT(f.tab->pageStatus(), QStringLiteral("Page 1 (3 / 8)"), kWaitMs);
    }

    void withoutLabelsThePageStatusKeepsItsPlainForm() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QTRY_VERIFY_WITH_TIMEOUT(f.controller()->pageCount() == 3, kWaitMs);
        f.controller()->goToPage(1);
        QTRY_COMPARE_WITH_TIMEOUT(f.tab->pageStatus(), QStringLiteral("Page 2 / 3"), kWaitMs);
        QVERIFY(!f.tab->hasPageLabels());
        QCOMPARE(f.tab->pageLabel(1), QStringLiteral("2"));
    }

    void goToPageAcceptsLabelsAndNumbers() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->hasPageLabels(), kWaitMs);

        f.tab->goToPageText(QStringLiteral(" ii "));
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 1U, kWaitMs);
        f.tab->goToPageText(QStringLiteral("A-2"));
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 6U, kWaitMs);
        // "3" is the label of the fifth page, not the third.
        f.tab->goToPageText(QStringLiteral("3"));
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 4U, kWaitMs);
        // No page is labelled "8", so it is read as a page number.
        f.tab->goToPageText(QStringLiteral("8"));
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 7U, kWaitMs);
        QVERIFY(f.tab->history().backCount() >= 4);
    }

    void aPageThatDoesNotExistIsSaidSoAndNothingMoves() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->hasPageLabels(), kWaitMs);
        QSignalSpy messages(f.tab, &vellora::DocumentTab::message);
        f.tab->goToPageText(QStringLiteral("99"));
        QTRY_VERIFY_WITH_TIMEOUT(messages.size() >= 1, kWaitMs);
        QVERIFY(messages.last().first().toString().contains(QStringLiteral("99")));
        QCOMPARE(f.controller()->currentPage(), 0U);
        QCOMPARE(f.tab->history().backCount(), 0);
        // Blank text asks nothing.
        f.tab->goToPageText(QStringLiteral("   "));
        QCOMPARE(messages.size(), 1);
    }

    void theGoToPageDialogStartsFromTheCurrentLabelAndGoesWhereItIsTold() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, QStringLiteral("doc.pdf"), documented()));
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->hasPageLabels(), kWaitMs);
        f.controller()->goToPage(5);
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 5U, kWaitMs);

        QString offeredLabel;
        quint32 offeredCount = 0;
        f.window.setGoToPageProvider([&](const QString& current, quint32 count) {
            offeredLabel = current;
            offeredCount = count;
            return std::optional<QString>(QStringLiteral("ii"));
        });
        QVERIFY(f.window.commands().run(QStringLiteral("view.goToPage")));
        QCOMPARE(offeredLabel, QStringLiteral("A-1"));
        QCOMPARE(offeredCount, 8U);
        QTRY_COMPARE_WITH_TIMEOUT(f.controller()->currentPage(), 1U, kWaitMs);

        // Cancelling goes nowhere.
        f.window.setGoToPageProvider(
            [](const QString&, quint32) { return std::optional<QString>(); });
        QVERIFY(f.window.commands().run(QStringLiteral("view.goToPage")));
        QTest::qWait(100);
        QCOMPARE(f.controller()->currentPage(), 1U);
    }
};

VELLORA_TEST_MAIN(TstNavigation)

#include "tst_navigation.moc"
