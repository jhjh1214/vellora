// PageLayout: pure geometry of the page column.
#include "canvas/PageLayout.h"

#include <QTest>
#include <cmath>
#include <limits>
#include <tuple>
#include <utility>

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

    void whatTheEngineReportsIsBounded() {
        PageLayout layout;
        // An absurd page count is cut instead of allocating gigabytes.
        layout.setPages(0xFFFFFFFFU, {});
        QCOMPARE(layout.pageCount(), PageLayout::kMaxPages);

        // Sizes that are not finite or not plausible are not believed.
        const double inf = std::numeric_limits<double>::infinity();
        const double nan = std::numeric_limits<double>::quiet_NaN();
        layout.setPages(6, {{100, 200}, {inf, 50}, {nan, 50}, {1e12, 50}, {-5, 50}, {0.5, 50}});
        for (quint32 page = 1; page < 6; ++page) {
            QCOMPARE(layout.pageSize(page), QSizeF(100, 200));
        }
        QCOMPARE(layout.maxPageWidth(), 100.0);
        QVERIFY(std::isfinite(layout.totalHeight(8.0)));

        // With no sane size at all, Letter.
        layout.setPages(2, {{inf, inf}, {nan, nan}});
        QCOMPARE(layout.pageSize(1),
                 QSizeF(PageLayout::kFallbackWidth, PageLayout::kFallbackHeight));
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

    // ---- spreads and rotation (M1 task 10) ----

    void twoUpPlacesPagesBesideTheSpine() {
        PageLayout layout;
        // Mixed sizes: 100x200, 300x100, 150x150, 80x400, 60x60.
        layout.setPages(5, {{100, 200}, {300, 100}, {150, 150}, {80, 400}, {60, 60}});
        layout.setArrangement(PageLayout::Spread::Two, 0);
        QCOMPARE(layout.rowCount(), 3U);
        for (const auto& [page, row, first, count] :
             {std::tuple<quint32, quint32, quint32, quint32>{0, 0, 0, 2},
              {1, 0, 0, 2},
              {2, 1, 2, 2},
              {3, 1, 2, 2},
              {4, 2, 4, 1}}) {
            QCOMPARE(layout.rowOf(page), row);
            QCOMPARE(layout.firstPageOf(row), first);
            QCOMPARE(layout.pagesInRow(row), count);
        }
        // A row is as tall as its tallest page; rows follow each other with the gap between.
        QCOMPARE(layout.rowHeightPoints(0), 200.0);
        QCOMPARE(layout.rowHeightPoints(1), 400.0);
        QCOMPARE(layout.rowHeightPoints(2), 60.0);
        QCOMPARE(layout.rowTop(0, 1.0), 12.0);
        QCOMPARE(layout.rowTop(1, 1.0), 12.0 + 200.0 + 12.0);
        QCOMPARE(layout.rowTop(2, 1.0), 12.0 + 200.0 + 12.0 + 400.0 + 12.0);
        QCOMPARE(layout.totalHeight(1.0), 4 * 12.0 + 660.0);

        // The spine is the centre line: the left page ends half a gap before it, the right page
        // starts half a gap after it, whatever their widths. Pages are aligned at the top of the
        // row.
        QCOMPARE(layout.pageRect(0, 1.0), QRectF(-6.0 - 100.0, 12.0, 100.0, 200.0));
        QCOMPARE(layout.pageRect(1, 1.0), QRectF(6.0, 12.0, 300.0, 100.0));
        QCOMPARE(layout.pageRect(2, 2.0), QRectF(-6.0 - 300.0, 12.0 + 400.0 + 12.0, 300.0, 300.0));
        // The last page has no partner and stays on the left, where an even page goes.
        QCOMPARE(layout.pageRect(4, 1.0).right(), -6.0);

        // What the column needs on each side of the spine is what the widest page on that side
        // needs: left pages are 100, 150 and 60 wide, right pages 300 and 80, so 300; and half the
        // gap stays fixed.
        QCOMPARE(layout.halfWidthPoints(), 300.0);
        QCOMPARE(layout.halfFixedWidth(), 6.0);
    }

    void theCoverPageStandsAloneOnTheRight() {
        PageLayout layout;
        layout.setPages(6,
                        {{100, 100}, {120, 100}, {130, 100}, {140, 100}, {150, 100}, {160, 100}});
        layout.setArrangement(PageLayout::Spread::TwoCover, 0);
        QCOMPARE(layout.rowCount(), 4U);
        // Rows: (0) (1 2) (3 4) (5).
        QCOMPARE(layout.pagesInRow(0), 1U);
        QCOMPARE(layout.firstPageOf(1), 1U);
        QCOMPARE(layout.firstPageOf(2), 3U);
        QCOMPARE(layout.firstPageOf(3), 5U);
        QCOMPARE(layout.pagesInRow(3), 1U);
        QCOMPARE(layout.rowOf(2), 1U);
        QCOMPARE(layout.rowOf(3), 2U);
        QCOMPARE(layout.rowOf(5), 3U);
        QCOMPARE(layout.pageRect(0, 1.0).left(), 6.0);   // the cover is right of the spine
        QCOMPARE(layout.pageRect(1, 1.0).right(), -6.0); // then pairs: odd pages on the left
        QCOMPARE(layout.pageRect(2, 1.0).left(), 6.0);
        QCOMPARE(layout.pageRect(5, 1.0).right(), -6.0); // an odd last page is alone on the left

        // How many rows a document of n pages has.
        for (const auto& [pages, rows] :
             {std::pair<quint32, quint32>{0, 0}, {1, 1}, {2, 2}, {3, 2}, {4, 3}, {5, 3}, {6, 4}}) {
            PageLayout l;
            l.setPages(pages, {});
            l.setArrangement(PageLayout::Spread::TwoCover, 0);
            QCOMPARE(l.rowCount(), rows);
            l.setArrangement(PageLayout::Spread::Two, 0);
            QCOMPARE(l.rowCount(), (pages + 1) / 2);
            l.setArrangement(PageLayout::Spread::One, 0);
            QCOMPARE(l.rowCount(), pages);
        }
    }

    void everyPageIsInExactlyOneRowAndRowsDoNotOverlap() {
        for (const auto spread :
             {PageLayout::Spread::One, PageLayout::Spread::Two, PageLayout::Spread::TwoCover}) {
            for (const int rotation : {0, 1, 2, 3}) {
                PageLayout layout;
                layout.setPages(9,
                                {{100, 200}, {200, 100}, {50, 80}, {300, 300}, {10, 20}, {99, 77}});
                layout.setArrangement(spread, rotation);
                quint32 pages = 0;
                double bottom = 0.0;
                for (quint32 row = 0; row < layout.rowCount(); ++row) {
                    QCOMPARE(layout.firstPageOf(row), pages);
                    pages += layout.pagesInRow(row);
                    QVERIFY(layout.rowTop(row, 1.5) >= bottom + PageLayout::kGap - 1e-9);
                    bottom = layout.rowTop(row, 1.5) + layout.rowHeight(row, 1.5);
                    for (quint32 i = 0; i < layout.pagesInRow(row); ++i) {
                        const quint32 page = layout.firstPageOf(row) + i;
                        QCOMPARE(layout.rowOf(page), row);
                        const QRectF rect = layout.pageRect(page, 1.5);
                        QVERIFY(rect.top() >= layout.rowTop(row, 1.5) - 1e-9);
                        QVERIFY(rect.bottom() <= bottom + 1e-9);
                        // Inside the column the widest row defines.
                        QVERIFY(rect.right() <=
                                layout.halfWidthPoints() * 1.5 + layout.halfFixedWidth() + 1e-9);
                        QVERIFY(rect.left() >=
                                -(layout.halfWidthPoints() * 1.5 + layout.halfFixedWidth()) - 1e-9);
                    }
                }
                QCOMPARE(pages, 9U);
                QCOMPARE(layout.totalHeight(1.5), bottom + PageLayout::kGap);
            }
        }
    }

    void twoPagesOfASpreadNeverOverlapHorizontally() {
        PageLayout layout;
        layout.setPages(4, {{500, 100}, {500, 100}, {20, 100}, {900, 100}});
        for (const auto spread : {PageLayout::Spread::Two, PageLayout::Spread::TwoCover}) {
            layout.setArrangement(spread, 0);
            for (quint32 row = 0; row < layout.rowCount(); ++row) {
                if (layout.pagesInRow(row) == 2) {
                    const quint32 first = layout.firstPageOf(row);
                    const QRectF left = layout.pageRect(first, 1.0);
                    const QRectF right = layout.pageRect(first + 1, 1.0);
                    QCOMPARE(right.left() - left.right(), PageLayout::kGap);
                }
            }
        }
    }

    void rotationSwapsTheShownSizeAndNotTheFile() {
        PageLayout layout;
        layout.setPages(2, {{100, 200}, {300, 50}});
        layout.setArrangement(PageLayout::Spread::One, 1);
        QCOMPARE(layout.rotation(), 1);
        QCOMPARE(layout.pageSize(0), QSizeF(100, 200)); // as in the file
        QCOMPARE(layout.displaySize(0), QSizeF(200, 100));
        QCOMPARE(layout.displaySize(1), QSizeF(50, 300));
        QCOMPARE(layout.maxPageWidth(), 200.0);
        QCOMPARE(layout.totalHeight(1.0), 3 * 12.0 + 100.0 + 300.0);
        QCOMPARE(layout.pageRect(1, 1.0), QRectF(-25.0, 12.0 + 100.0 + 12.0, 50.0, 300.0));
        // Half turns keep the sizes; any integer is a number of quarter turns.
        layout.setArrangement(PageLayout::Spread::One, 2);
        QCOMPARE(layout.displaySize(0), QSizeF(100, 200));
        layout.setArrangement(PageLayout::Spread::One, -1);
        QCOMPARE(layout.rotation(), 3);
        layout.setArrangement(PageLayout::Spread::One, 5);
        QCOMPARE(layout.rotation(), 1);
        layout.setArrangement(PageLayout::Spread::One, 4);
        QCOMPARE(layout.rotation(), 0);
        QCOMPARE(layout.displaySize(1), QSizeF(300, 50));
    }

    void rotatedPagesInASpreadUseTheirShownSizes() {
        PageLayout layout;
        layout.setPages(2, {{100, 200}, {300, 100}});
        layout.setArrangement(PageLayout::Spread::Two, 1);
        // Shown: 200 x 100 and 100 x 300. The row is as tall as the tallest, 300.
        QCOMPARE(layout.rowHeightPoints(0), 300.0);
        QCOMPARE(layout.rowWidthPoints(0), 300.0);
        QCOMPARE(layout.rowFixedWidth(0), PageLayout::kGap);
        QCOMPARE(layout.pageRect(0, 1.0), QRectF(-6.0 - 200.0, 12.0, 200.0, 100.0));
        QCOMPARE(layout.pageRect(1, 1.0), QRectF(6.0, 12.0, 100.0, 300.0));
        QCOMPARE(layout.halfWidthPoints(), 200.0);
        QCOMPARE(layout.halfFixedWidth(), 6.0);
    }

    void rotatingAPointOfAPageMovesItAroundTheCorners() {
        const QSizeF file(100, 200);
        const QRectF corner(0, 0, 10, 20); // the top left corner of the page as in the file
        // A quarter turn clockwise takes the top left corner to the top right, ...
        QCOMPARE(PageLayout::toScreen(corner, file, 0, 1.0), QRectF(0, 0, 10, 20));
        QCOMPARE(PageLayout::toScreen(corner, file, 1, 1.0), QRectF(180, 0, 20, 10));
        QCOMPARE(PageLayout::toScreen(corner, file, 2, 1.0), QRectF(90, 180, 10, 20));
        QCOMPARE(PageLayout::toScreen(corner, file, 3, 1.0), QRectF(0, 90, 20, 10));
        // ...and the zoom scales it.
        QCOMPARE(PageLayout::toScreen(QRectF(10, 20, 30, 40), file, 1, 2.0),
                 QRectF(2 * (200 - 60), 2 * 10, 2 * 40, 2 * 30));
    }

    void toPageIsTheInverseOfToScreenForEveryTurn() {
        const QSizeF file(120, 340);
        for (int rotation = 0; rotation < 4; ++rotation) {
            for (const double zoom : {0.25, 1.0, 3.5}) {
                for (const QRectF rect : {QRectF(0, 0, 120, 340), QRectF(10, 20, 30, 40),
                                          QRectF(100, 300, 20, 40), QRectF(5, 5, 1, 1)}) {
                    const QRectF there = PageLayout::toScreen(rect, file, rotation, zoom);
                    const QRectF back = PageLayout::toPage(there, file, rotation, zoom);
                    QVERIFY2(
                        qAbs(back.x() - rect.x()) < 1e-9 && qAbs(back.y() - rect.y()) < 1e-9 &&
                            qAbs(back.width() - rect.width()) < 1e-9 &&
                            qAbs(back.height() - rect.height()) < 1e-9,
                        qPrintable(QStringLiteral("rotation %1 zoom %2").arg(rotation).arg(zoom)));
                }
            }
            // The whole page lands exactly on the shown page rectangle.
            const QRectF whole =
                PageLayout::toScreen(QRectF(QPointF(0, 0), file), file, rotation, 1.0);
            const QSizeF shown = rotation % 2 == 0 ? file : QSizeF(file.height(), file.width());
            QCOMPARE(whole, QRectF(QPointF(0, 0), shown));
        }
    }

    void pagesAndAnchorsFollowRowsInASpread() {
        PageLayout layout;
        layout.setPages(5, {{100, 200}, {100, 100}, {100, 100}, {100, 300}, {100, 50}});
        layout.setArrangement(PageLayout::Spread::Two, 0);
        // pageTop is the top of the page's row; pageAt names the first page of the row.
        QCOMPARE(layout.pageTop(1, 1.0), layout.pageTop(0, 1.0));
        QCOMPARE(layout.pageAt(layout.rowTop(1, 1.0) + 5.0, 1.0), 2U);
        // Both pages of a spread are visible together.
        const auto range = layout.pagesIn(0.0, layout.rowTop(1, 1.0) - 1.0, 1.0);
        QCOMPARE(range.first, 0U);
        QCOMPARE(range.count, 2U);
        const auto all = layout.pagesIn(0.0, 1.0e6, 1.0);
        QCOMPARE(all.first, 0U);
        QCOMPARE(all.count, 5U);
        // Rows past the end do not exist.
        QCOMPARE(layout.pagesInRow(3), 0U);
        QCOMPARE(layout.rowOf(5), 3U);
        // An anchor is a place in a row and keeps its distance in points through a zoom.
        const double y = layout.rowTop(1, 1.0) + 40.0;
        const auto anchor = layout.anchorAt(y, 1.0);
        QCOMPARE(anchor.page, 2U);
        QCOMPARE(anchor.offsetPoints, 40.0);
        QCOMPARE(layout.yOf(anchor, 2.0), layout.rowTop(1, 2.0) + 80.0);
    }

    void fitFiguresDescribeTheWidestRow() {
        PageLayout one;
        one.setPages(3, {{100, 100}, {400, 100}, {250, 100}});
        QCOMPARE(one.halfWidthPoints(), 200.0);
        QCOMPARE(one.halfFixedWidth(), 0.0);
        QCOMPARE(one.rowWidthPoints(1), 400.0);
        QCOMPARE(one.rowFixedWidth(1), 0.0);

        PageLayout two;
        two.setPages(3, {{100, 100}, {400, 100}, {250, 100}});
        two.setArrangement(PageLayout::Spread::Two, 0);
        QCOMPARE(two.rowWidthPoints(0), 500.0);
        QCOMPARE(two.rowFixedWidth(0), PageLayout::kGap);
        QCOMPARE(two.rowWidthPoints(1), 250.0);
        QCOMPARE(two.rowFixedWidth(1), 0.0); // a lone page has no neighbour to leave room for
        QCOMPARE(two.halfWidthPoints(), 400.0);
    }

    void aHugeDocumentIsCheapInEverySpread() {
        for (const auto spread :
             {PageLayout::Spread::One, PageLayout::Spread::Two, PageLayout::Spread::TwoCover}) {
            PageLayout layout;
            layout.setPages(10'000, {});
            layout.setArrangement(spread, 1);
            const double height = layout.totalHeight(1.0);
            QVERIFY(height > 10'000 * 100.0);
            const quint32 page = layout.pageAt(height / 2.0, 1.0);
            QVERIFY(layout.pageTop(page, 1.0) <= height / 2.0);
            QCOMPARE(layout.pageAt(height * 2.0, 1.0), layout.firstPageOf(layout.rowCount() - 1));
        }
    }
};
QTEST_APPLESS_MAIN(TstPageLayout)
#include "tst_page_layout.moc"
