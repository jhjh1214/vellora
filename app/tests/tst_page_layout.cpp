// PageLayout: pure geometry of the page column.
#include "canvas/PageLayout.h"

#include <QTest>

using vellora::PageLayout;

class TstPageLayout : public QObject {
    Q_OBJECT

private slots:
    void positionsFollowSizesAndGaps() {
        PageLayout layout;
        layout.setPages(3, {{100, 200}, {200, 100}, {100, 100}});
        QCOMPARE(layout.pageCount(), 3U);
        QCOMPARE(layout.maxPageWidth(), 200.0);
        // Gap above the first page, then page, gap, page, ...
        QCOMPARE(layout.pageTop(0, 1.0), 12.0);
        QCOMPARE(layout.pageTop(1, 1.0), 12.0 + 200.0 + 12.0);
        QCOMPARE(layout.pageTop(2, 1.0), 12.0 + 200.0 + 12.0 + 100.0 + 12.0);
        QCOMPARE(layout.totalHeight(1.0), 4 * 12.0 + 400.0);
        // The gap does not scale with the zoom, the pages do.
        QCOMPARE(layout.pageTop(1, 2.0), 24.0 + 400.0);
        QCOMPARE(layout.totalHeight(2.0), 4 * 12.0 + 800.0);
    }

    void pageAtFindsTheLastPageStartingAtOrAboveY() {
        PageLayout layout;
        layout.setPages(3, {{100, 200}, {200, 100}, {100, 100}});
        QCOMPARE(layout.pageAt(-50.0, 1.0), 0U);
        QCOMPARE(layout.pageAt(0.0, 1.0), 0U);
        QCOMPARE(layout.pageAt(223.9, 1.0), 0U);
        QCOMPARE(layout.pageAt(224.0, 1.0), 1U);
        QCOMPARE(layout.pageAt(335.9, 1.0), 1U);
        QCOMPARE(layout.pageAt(336.0, 1.0), 2U);
        QCOMPARE(layout.pageAt(1.0e9, 1.0), 2U);
    }

    void pagesInReturnsTheIntersectingPages() {
        PageLayout layout;
        layout.setPages(3, {{100, 200}, {200, 100}, {100, 100}});
        const auto range = [&](double top, double bottom) {
            return layout.pagesIn(top, bottom, 1.0);
        };
        const auto same = [](PageLayout::Range a, quint32 first, quint32 count) {
            return a.first == first && a.count == count;
        };
        QVERIFY(same(range(0, 100), 0, 1));
        QVERIFY(same(range(0, 448), 0, 3));
        QVERIFY(same(range(215, 230), 1, 1));   // bottom of page 0 is 212; page 1 starts at 224
        QVERIFY(same(range(224, 336), 1, 1));   // page 2 starts at 336, not before
        QVERIFY(same(range(212, 224), 0, 0));   // only the gap between pages 0 and 1
        QVERIFY(same(range(300, 300), 0, 0));   // empty range
        QVERIFY(same(range(1000, 2000), 0, 0)); // below the document
        QCOMPARE(layout.pagesIn(0, 100, 1.0).count, 1U);
    }

    void unknownSizesTakeTheLastKnownPageOrLetter() {
        PageLayout layout;
        layout.setPages(5, {{100, 200}, {300, 400}});
        QCOMPARE(layout.pageSize(1), QSizeF(300, 400));
        QCOMPARE(layout.pageSize(4), QSizeF(300, 400));
        QCOMPARE(layout.maxPageWidth(), 300.0);

        layout.setPages(2, {});
        QCOMPARE(layout.pageSize(0),
                 QSizeF(PageLayout::kFallbackWidth, PageLayout::kFallbackHeight));

        // A page the engine could not measure arrives as an invalid size.
        layout.setPages(3, {{100, 200}, QSizeF(), {50, 60}});
        QCOMPARE(layout.pageSize(1), QSizeF(50, 60));

        // More sizes than pages are ignored.
        layout.setPages(1, {{10, 20}, {30, 40}});
        QCOMPARE(layout.pageCount(), 1U);
        QCOMPARE(layout.pageSize(0), QSizeF(10, 20));
    }

    void anAnchorSurvivesAZoomChange() {
        PageLayout layout;
        layout.setPages(3, {{100, 200}, {200, 100}, {100, 100}});
        for (const double y : {0.0, 100.0, 230.0, 300.0, 400.0}) {
            const auto anchor = layout.anchorAt(y, 1.0);
            QVERIFY2(qAbs(layout.yOf(anchor, 1.0) - y) < 1e-9, qPrintable(QString::number(y)));
        }
        // 26 points below the top of page 1 stays 26 points below it at 200%.
        const auto anchor = layout.anchorAt(250.0, 1.0);
        QCOMPARE(anchor.page, 1U);
        QCOMPARE(anchor.offsetPoints, 26.0);
        QCOMPARE(layout.yOf(anchor, 2.0), layout.pageTop(1, 2.0) + 52.0);
    }

    void anEmptyDocumentHasNoPages() {
        PageLayout layout;
        QCOMPARE(layout.pageCount(), 0U);
        QCOMPARE(layout.pageAt(10.0, 1.0), 0U);
        QCOMPARE(layout.pagesIn(0, 100, 1.0).count, 0U);
        QCOMPARE(layout.totalHeight(1.0), 12.0);
    }

    void tenThousandPagesAreCheap() {
        PageLayout layout;
        layout.setPages(10'000, {});
        const double height = layout.totalHeight(1.0);
        QCOMPARE(height, 12.0 * 10'001 + PageLayout::kFallbackHeight * 10'000);
        QCOMPARE(layout.pageAt(height, 1.0), 9'999U);
        const quint32 middle = layout.pageAt(height / 2.0, 1.0);
        QVERIFY(layout.pageTop(middle, 1.0) <= height / 2.0);
        QVERIFY(layout.pageTop(middle + 1, 1.0) > height / 2.0);
    }
};

QTEST_APPLESS_MAIN(TstPageLayout)
#include "tst_page_layout.moc"
