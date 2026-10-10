// M1 task 15b: the find bar, the hits painted over the pages, the list of results and stepping
// through them, with a real engine and no GPU. The documents are small synthetic files with
// Helvetica text.
#include "KillProcess.h"
#include "MainWindow.h"
#include "SyntheticPdf.h"
#include "TextPdf.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"
#include "search/FindBar.h"
#include "search/SearchOverlay.h"
#include "search/SearchResultsView.h"
#include "sidebar/ThumbnailSidebar.h"

#include <QCheckBox>
#include <QDir>
#include <QFile>
#include <QLabel>
#include <QLineEdit>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <QToolButton>

using vellora::SearchController;

namespace {

constexpr int kWaitMs = 60'000;

// One line of text at `y` points from the bottom.
QString line(const QString& text, int y = 700) {
    return QStringLiteral("BT /F1 20 Tf 72 %1 Td (%2) Tj ET").arg(y).arg(text);
}

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    vellora::DocumentTab* tab = nullptr;

    explicit Fixture(const QByteArray& pdf, double zoom = 1.0) {
        const QString path = dir.filePath(QStringLiteral("search.pdf"));
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
        controller()->setZoom(zoom, QPointF(0.0, 0.0));
        controller()->goToPage(0);
    }

    vellora::CanvasController* controller() { return window.canvas().controller(); }
    SearchController& search() { return tab->search(); }
    vellora::FindBar& bar() { return tab->findBar(); }

    // Opens the find bar and searches for `text` with the given options; returns when the search
    // is over.
    void find(const QString& text, bool matchCase = false, bool words = false, bool regex = false) {
        bar().open();
        bar().matchCase().setChecked(matchCase);
        bar().wholeWords().setChecked(words);
        bar().regex().setChecked(regex);
        bar().edit().setText(text);
        bar().searchNow();
        waitForEnd();
    }

    void waitForEnd() {
        QVERIFY2(QTest::qWaitFor([this] { return !search().isRunning(); }, kWaitMs),
                 "the search did not end");
    }

    // The pages of every hit, in order.
    QList<quint32> pages() {
        QList<quint32> out;
        for (const vellora::SearchHit& hit : search().hits()) {
            out.append(hit.page);
        }
        return out;
    }
};

// Three pages: a word on the first, two on the second, one on the third.
QByteArray needles() {
    return textPdf({line(QStringLiteral("Alpha beta")),
                    line(QStringLiteral("a needle in a haystack"), 700) +
                        line(QStringLiteral("and another needle here"), 650),
                    line(QStringLiteral("Gamma needle"))});
}

} // namespace

class SearchTest : public QObject {
    Q_OBJECT

private slots:
    void hits_are_found_counted_and_painted() {
        Fixture f(needles(), 0.3);
        f.find(QStringLiteral("needle"));
        QCOMPARE(f.search().state(), SearchController::State::Finished);
        QCOMPARE(f.search().count(), 3);
        QCOMPARE(f.pages(), (QList<quint32>{1, 1, 2}));
        QCOMPARE(f.search().pagesDone(), 3U);
        const vellora::SearchHit& first = f.search().hits().first();
        QCOMPARE(first.snippet.mid(first.matchStart, first.matchLength), QStringLiteral("needle"));
        QCOMPARE(first.count, 6U);
        QCOMPARE(first.boxes.size(), 1);
        QCOMPARE(f.search().statusText(), QStringLiteral("1 of 3 result(s)"));
        QCOMPARE(f.bar().status().text(), QStringLiteral("1 of 3 result(s)"));

        // Every hit is painted on its page, the current one differently.
        const QList<vellora::SearchOverlay::Highlight> marks =
            f.window.canvas().searchOverlay()->highlights();
        QCOMPARE(marks.size(), 3);
        int current = 0;
        for (const auto& mark : marks) {
            current += mark.current ? 1 : 0;
            QVERIFY(f.window.canvas().viewport()->rect().intersects(mark.rect.toRect()));
        }
        QCOMPARE(current, 1);
    }

    void next_and_previous_step_and_wrap() {
        Fixture f(needles());
        f.find(QStringLiteral("needle"));
        QCOMPARE(f.search().current(), 0);
        f.tab->findNext();
        QCOMPARE(f.search().current(), 1);
        f.tab->findNext();
        QCOMPARE(f.search().current(), 2);
        f.tab->findNext();
        QCOMPARE(f.search().current(), 0); // wraps
        f.tab->findPrevious();
        QCOMPARE(f.search().current(), 2);
        f.tab->findPrevious();
        QCOMPARE(f.search().current(), 1);
        QCOMPARE(f.bar().status().text(), QStringLiteral("2 of 3 result(s)"));
        // The buttons and the keys do the same.
        QTest::mouseClick(&f.bar().nextButton(), Qt::LeftButton);
        QCOMPARE(f.search().current(), 2);
        QTest::mouseClick(&f.bar().previousButton(), Qt::LeftButton);
        QCOMPARE(f.search().current(), 1);
        QTest::keyClick(&f.bar().edit(), Qt::Key_Return);
        QCOMPARE(f.search().current(), 2);
        QTest::keyClick(&f.bar().edit(), Qt::Key_Return, Qt::ShiftModifier);
        QCOMPARE(f.search().current(), 1);
    }

    void the_view_goes_to_the_current_hit() {
        QStringList pages;
        for (int i = 0; i < 12; ++i) {
            pages << line(
                i == 8 ? QStringLiteral("the target word") : QStringLiteral("nothing here"), 400);
        }
        Fixture f(textPdf(pages));
        QCOMPARE(f.controller()->currentPage(), 0U);
        f.find(QStringLiteral("target"));
        QCOMPARE(f.search().count(), 1);
        QCOMPARE(f.controller()->currentPage(), 8U);
        const auto marks = f.window.canvas().searchOverlay()->highlights();
        QCOMPARE(marks.size(), 1);
        const QRect viewport = f.window.canvas().viewport()->rect();
        QVERIFY2(viewport.contains(marks.first().rect.toRect()), "the hit is in view");
        // Stepping back to a hit far away brings the view back to it.
        f.controller()->goToPage(0);
        f.tab->findNext();
        QCOMPARE(f.controller()->currentPage(), 8U);
    }

    void searching_starts_at_the_page_on_screen_and_wraps() {
        Fixture f(textPdf({line(QStringLiteral("needle")), line(QStringLiteral("nothing")),
                           line(QStringLiteral("needle again")), line(QStringLiteral("empty"))}));
        f.controller()->goToPage(1);
        f.find(QStringLiteral("needle"));
        QCOMPARE(f.search().count(), 2);
        QCOMPARE(f.search().current(), 1); // the hit on page 3, the first after page 2
        f.controller()->goToPage(3);
        f.find(QStringLiteral("needle "), false, false, false); // a different query
        QCOMPARE(f.search().count(), 1);
        QCOMPARE(f.search().current(), 0); // none after page 4: the first of the document
    }

    void the_options_change_what_is_found() {
        Fixture f(textPdf({line(QStringLiteral("Needle needle NEEDLE needlework"))}));
        f.find(QStringLiteral("needle"));
        QCOMPARE(f.search().count(), 4);
        f.find(QStringLiteral("needle"), true);
        QCOMPARE(f.search().count(), 2); // needle, and the start of needlework
        f.find(QStringLiteral("needle"), false, true);
        QCOMPARE(f.search().count(), 3); // not needlework
        f.find(QStringLiteral("need.e\\w*"), false, false, true);
        QCOMPARE(f.search().count(), 4);
        f.find(QStringLiteral("need.e\\w*"), false, false, false);
        QCOMPARE(f.search().count(), 0); // plain text is not an expression
        QCOMPARE(f.search().statusText(), QStringLiteral("No results"));
    }

    void an_expression_that_does_not_parse_says_why() {
        Fixture f(needles());
        f.find(QStringLiteral("(unclosed"), false, false, true);
        QCOMPARE(f.search().state(), SearchController::State::Failed);
        QVERIFY(!f.search().error().isEmpty());
        QCOMPARE(f.bar().status().text(), f.search().error());
        QCOMPARE(f.search().count(), 0);
        QVERIFY(f.window.canvas().searchOverlay()->highlights().isEmpty());
        // Mending the expression searches again.
        f.find(QStringLiteral("(need)le"), false, false, true);
        QCOMPARE(f.search().state(), SearchController::State::Finished);
        QCOMPARE(f.search().count(), 3);
    }

    void a_new_query_replaces_the_old_results() {
        Fixture f(needles());
        f.search().start({QStringLiteral("a"), false, false, false});
        f.search().start({QStringLiteral("Gamma"), false, false, false});
        f.waitForEnd();
        QCOMPARE(f.search().query().text, QStringLiteral("Gamma"));
        QCOMPARE(f.pages(), (QList<quint32>{2}));
        QCOMPARE(f.search().count(), 1);
    }

    void typing_searches_after_a_pause() {
        Fixture f(needles());
        f.bar().open();
        QTest::keyClicks(&f.bar().edit(), QStringLiteral("need"));
        // Nothing yet: the text has not rested.
        QCOMPARE(f.search().state(), SearchController::State::Idle);
        QVERIFY(QTest::qWaitFor(
            [&f] { return f.search().state() != SearchController::State::Idle; }, kWaitMs));
        f.waitForEnd();
        QCOMPARE(f.search().count(), 3);
        QTest::keyClicks(&f.bar().edit(), QStringLiteral("le"));
        QVERIFY(QTest::qWaitFor([&f] { return f.search().query().text == QLatin1String("needle"); },
                                kWaitMs));
        f.waitForEnd();
        QCOMPARE(f.search().count(), 3);
        // Emptying the field forgets the results.
        f.bar().edit().clear();
        QCOMPARE(f.search().state(), SearchController::State::Idle);
        QCOMPARE(f.search().count(), 0);
    }

    void escape_closes_the_bar_and_clears_the_hits() {
        Fixture f(needles());
        f.find(QStringLiteral("needle"));
        QVERIFY(!f.bar().isHidden());
        QVERIFY(!f.window.canvas().searchOverlay()->highlights().isEmpty());
        QSignalSpy closed(&f.bar(), &vellora::FindBar::closed);
        QTest::keyClick(&f.bar().edit(), Qt::Key_Escape);
        QVERIFY(f.bar().isHidden());
        QCOMPARE(closed.count(), 1);
        QCOMPARE(f.search().state(), SearchController::State::Idle);
        QCOMPARE(f.search().count(), 0);
        QVERIFY(f.window.canvas().searchOverlay()->highlights().isEmpty());
        // F3 with the bar closed opens it.
        f.tab->findNext();
        QVERIFY(!f.bar().isHidden());
    }

    void the_list_shows_every_hit_and_choosing_one_goes_there() {
        Fixture f(needles());
        f.find(QStringLiteral("needle"));
        f.tab->showSearchResults();
        QVERIFY(f.tab->sidebarVisible());
        vellora::SearchResultsView* results = f.tab->sidebar().results();
        QVERIFY(results != nullptr);
        QCOMPARE(f.tab->sidebar().currentTab(), vellora::ThumbnailSidebar::Tab::Search);
        vellora::SearchResultsModel& model = results->model();
        QCOMPARE(model.rowCount(), 3);
        QCOMPARE(model.index(0).data(vellora::SearchResultsModel::MatchRole).toString(),
                 QStringLiteral("needle"));
        QCOMPARE(model.index(0).data(vellora::SearchResultsModel::PageLabelRole).toString(),
                 QStringLiteral("2"));
        QCOMPARE(model.index(2).data(vellora::SearchResultsModel::PageLabelRole).toString(),
                 QStringLiteral("3"));
        const QString before =
            model.index(1).data(vellora::SearchResultsModel::BeforeRole).toString();
        QVERIFY2(before.endsWith(QLatin1String("and another ")), qPrintable(before));
        QCOMPARE(results->summary().text(), QStringLiteral("1 of 3 result(s)"));
        QCOMPARE(results->list().currentIndex().row(), 0);

        // Choosing a hit makes it current, goes there and can be undone with Back.
        QVERIFY(!f.tab->history().canGoBack());
        const QModelIndex third = model.index(2);
        QTest::mouseClick(results->list().viewport(), Qt::LeftButton, {},
                          results->list().visualRect(third).center());
        QCOMPARE(f.search().current(), 2);
        QCOMPARE(f.controller()->currentPage(), 2U);
        QVERIFY(f.tab->history().canGoBack());
        // Stepping with the keys follows in the list.
        f.tab->findPrevious();
        QCOMPARE(results->list().currentIndex().row(), 1);
    }

    void the_list_grows_while_the_search_runs() {
        QStringList pages;
        for (int i = 0; i < 400; ++i) {
            pages << line(QStringLiteral("every page says needle"), 400);
        }
        Fixture f(textPdf(pages));
        f.bar().open();
        f.bar().edit().setText(QStringLiteral("needle"));
        f.bar().searchNow();
        // Rows appear before the search is over, in page order.
        vellora::SearchResultsModel& model = f.tab->sidebar().results()->model();
        QVERIFY(QTest::qWaitFor([&model] { return model.rowCount() > 0; }, kWaitMs));
        f.waitForEnd();
        QCOMPARE(model.rowCount(), 400);
        QCOMPARE(f.search().count(), 400);
        for (int i = 1; i < f.search().count(); ++i) {
            QVERIFY(f.search().hits().at(i - 1).page <= f.search().hits().at(i).page);
        }
        QCOMPARE(f.search().pagesDone(), 400U);
    }

    void a_document_without_the_words_has_no_results() {
        Fixture f(syntheticPdf(3));
        f.find(QStringLiteral("zebra"));
        QCOMPARE(f.search().state(), SearchController::State::Finished);
        QCOMPARE(f.search().count(), 0);
        QCOMPARE(f.search().statusText(), QStringLiteral("No results"));
        QVERIFY(!f.tab->findBar().nextButton().isHidden());
        f.tab->findNext(); // nothing to step to, and no harm
        QCOMPARE(f.search().current(), -1);
        // The words that are there are found.
        f.find(QStringLiteral("page 2"));
        QCOMPARE(f.pages(), (QList<quint32>{1}));
    }

    void the_commands_exist_with_their_shortcuts() {
        Fixture f(needles());
        const auto& commands = f.window.commands();
        for (const char* id :
             {"edit.find", "edit.findNext", "edit.findPrevious", "view.searchResults"}) {
            QVERIFY2(commands.find(QString::fromLatin1(id)) != nullptr, id);
        }
        QVERIFY(commands.find(QStringLiteral("edit.find"))
                    ->shortcuts.contains(QKeySequence(QKeySequence::Find)));
        QVERIFY(commands.find(QStringLiteral("edit.findNext"))
                    ->shortcuts.contains(QKeySequence(Qt::Key_F3)));
        QVERIFY(commands.find(QStringLiteral("edit.findPrevious"))
                    ->shortcuts.contains(QKeySequence(Qt::SHIFT | Qt::Key_F3)));
        QVERIFY(f.bar().isHidden());
        QVERIFY(commands.run(QStringLiteral("edit.find")));
        QVERIFY(!f.bar().isHidden());
        f.bar().edit().setText(QStringLiteral("needle"));
        f.bar().searchNow();
        f.waitForEnd();
        QVERIFY(commands.run(QStringLiteral("edit.findNext")));
        QCOMPARE(f.search().current(), 1);
        QVERIFY(commands.run(QStringLiteral("edit.findPrevious")));
        QCOMPARE(f.search().current(), 0);
        QVERIFY(commands.run(QStringLiteral("view.searchResults")));
        QCOMPARE(f.tab->sidebar().currentTab(), vellora::ThumbnailSidebar::Tab::Search);
    }

    void a_search_in_an_engine_that_dies_is_run_again_by_the_new_one() {
        QStringList pages;
        for (int i = 0; i < 3000; ++i) {
            pages << line(i == 2999 ? QStringLiteral("the needle at the end")
                                    : QStringLiteral("filler text"),
                          400);
        }
        Fixture f(textPdf(pages));
        QSignalSpy crashed(&f.tab->session(), &vellora::EngineSession::engineCrashed);
        QSignalSpy opened(&f.tab->session(), &vellora::EngineSession::opened);
        f.search().start({QStringLiteral("needle"), false, false, false});
        killProcess(f.tab->session().engineProcessId());
        QTRY_VERIFY_WITH_TIMEOUT(crashed.size() >= 1, kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(opened.size() >= 1, kWaitMs); // the new engine has the document
        QTRY_VERIFY_WITH_TIMEOUT(f.search().state() == SearchController::State::Finished, kWaitMs);
        // Whether the first engine had finished or not, the answer is complete and not doubled.
        QCOMPARE(f.search().count(), 1);
        QCOMPARE(f.pages(), (QList<quint32>{2999}));
    }

    void opening_another_document_forgets_the_search() {

        Fixture f(needles());
        f.find(QStringLiteral("needle"));
        QCOMPARE(f.search().count(), 3);
        const QString other = f.dir.filePath(QStringLiteral("other.pdf"));
        QFile file(other);
        QVERIFY(file.open(QIODevice::WriteOnly));
        file.write(textPdf({line(QStringLiteral("something else"))}));
        file.close();
        QSignalSpy opened(&f.tab->session(), &vellora::EngineSession::opened);
        QVERIFY(f.tab->open(other)); // this tab, which still shows the first document
        QVERIFY(opened.wait(kWaitMs));
        QCOMPARE(f.search().state(), SearchController::State::Idle);
        QCOMPARE(f.search().count(), 0);
        QVERIFY(f.bar().isHidden());
        QVERIFY(f.window.canvas().searchOverlay()->highlights().isEmpty());
    }
};

VELLORA_TEST_MAIN(SearchTest)
#include "tst_search.moc"
