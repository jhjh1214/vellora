// M1 task 13b: links in the viewer, with a real engine and no GPU. What is tested is what the
// reader sees and what happens when they click: the pointer and tool tip over a link, internal
// jumps (and the history), addresses only after a question and only for the allowed schemes, and
// that actions Vellora never runs are described and left alone.
#include "MainWindow.h"
#include "VelloraTestMain.h"
#include "canvas/CanvasController.h"
#include "links/LinkActions.h"
#include "links/UriConfirmDialog.h"

#include <QCheckBox>
#include <QFile>
#include <QPlainTextEdit>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>

using vellora::CanvasController;
using vellora::DocumentTab;
using vellora::LinkActions;

namespace {

constexpr int kWaitMs = 60'000;
constexpr int kPages = 4;
constexpr double kPageHeight = 792.0;

// A document of `kPages` pages of 612 x 792 points; the first has a link annotation of every kind.
QByteArray linkedPdf() {
    // The rectangles are 100 x 40 points, listed from the top of the page down; the second column
    // is at x = 300.
    struct Entry {
        double x;
        double y; // bottom, in user space
        QString action;
    };
    const QList<Entry> entries = {
        {100, 700, QStringLiteral("/Dest [4 0 R /Fit]")},
        {100, 600, QStringLiteral("/A << /S /URI /URI (https://example.org/path?q=1) >>")},
        {100, 500, QStringLiteral("/A << /S /URI /URI (javascript:alert(1)) >>")},
        {100, 400, QStringLiteral("/A << /S /URI /URI (http://trusted.example/a) >>")},
        {300, 400, QStringLiteral("/A << /S /URI /URI (http://trusted.example/b) >>")},
        {100, 300, QStringLiteral("/A << /S /URI /URI (mailto:me@example.org) >>")},
        {100, 200, QStringLiteral("/A << /S /Named /N /NextPage >>")},
        {100, 100, QStringLiteral("/A << /S /Launch /F (calc.exe) >>")},
        {300, 700, QStringLiteral("/Dest /nowhere")},
        {300, 600, QStringLiteral("/A << /S /JavaScript /JS (app.alert(1)) >>")},
        {300, 500, QStringLiteral("/A << /S /URI /URI (http://other.example/c) >>")},
    };
    QStringList annots;
    for (const Entry& e : entries) {
        annots << QStringLiteral("<< /Type /Annot /Subtype /Link /Rect [%1 %2 %3 %4] %5 >>")
                      .arg(e.x)
                      .arg(e.y)
                      .arg(e.x + 100)
                      .arg(e.y + 40)
                      .arg(e.action);
    }
    QStringList objects;
    objects << QStringLiteral("<< /Type /Catalog /Pages 2 0 R >>");
    objects << QStringLiteral("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count %1 >>")
                   .arg(kPages);
    objects << QStringLiteral(
                   "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [%1] >>")
                   .arg(annots.join(QLatin1Char(' ')));
    for (int i = 1; i < kPages; ++i) {
        objects << QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>");
    }
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

// The links' names, for reading the tests: the centre of each, in user space.
enum Link {
    Goto = 0,
    HttpsUri,
    JavascriptUri,
    TrustedA,
    TrustedB,
    Mailto,
    NextPage,
    Launch,
    Nowhere,
    JavaScriptAction,
    OtherHost,
};

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    DocumentTab* tab = nullptr;
    QList<QString> asked; // the addresses the question was asked about
    QList<QString> askedHosts;
    QList<QUrl> opened;
    QStringList notices;
    DocumentTab::UriChoice answer{true, false};

    Fixture() {
        QFile file(dir.filePath(QStringLiteral("links.pdf")));
        if (!file.open(QIODevice::WriteOnly)) {
            qFatal("cannot write the test document");
        }
        file.write(linkedPdf());
        file.close();
        window.resize(900, 700);
        window.show();
        tab = &window.currentTab();
        tab->setUriConfirmer([this](const QString& uri, const QString& host) {
            asked.append(uri);
            askedHosts.append(host);
            return answer;
        });
        tab->setUriOpener([this](const QUrl& url) {
            opened.append(url);
            return true;
        });
        tab->setNotifier([this](const QString& text) { notices.append(text); });
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        if (!window.openDocument(dir.filePath(QStringLiteral("links.pdf"))) ||
            !opened.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
        // The whole page fits, so that every link is on screen.
        controller()->setZoom(0.6, QPointF(0.0, 0.0));
        controller()->goToPage(0);
        if (!QTest::qWaitFor([this] { return links()->isComplete(0); }, kWaitMs)) {
            qFatal("the links of page 1 were not read");
        }
    }

    CanvasController* controller() { return window.canvas().controller(); }
    vellora::LinkLayer* links() { return window.canvas().links(); }
    vellora::CanvasView& view() { return window.canvas(); }

    // The viewport position of the centre of link `which` (user space x, y of its middle).
    QPoint centreOf(Link which) {
        static const QList<QPointF> middles = {
            {150, 720}, {150, 620}, {150, 520}, {150, 420}, {350, 420}, {150, 320},
            {150, 220}, {150, 120}, {350, 720}, {350, 620}, {350, 520},
        };
        const vellora::PageDraw page = controller()->frame().pages.first();
        const QPointF m = middles.at(which);
        return (page.rect.topLeft() +
                QPointF(m.x() * controller()->zoom(), (kPageHeight - m.y()) * controller()->zoom()))
            .toPoint();
    }

    void click(Link which) {
        QTest::mouseClick(view().viewport(), Qt::LeftButton, {}, centreOf(which));
    }
};

} // namespace

class TstLinks : public QObject {
    Q_OBJECT

private slots:
    // ---- the policy ----

    void onlyWebAndMailAddressesMayBeOpened_data() {
        QTest::addColumn<QString>("uri");
        QTest::addColumn<bool>("allowed");
        QTest::addColumn<QString>("host");
        QTest::newRow("https") << "https://Example.org/a?b=c#d" << true << "example.org";
        QTest::newRow("http, upper case scheme") << "HTTP://example.org" << true << "example.org";
        QTest::newRow("with a port and user")
            << "https://u@example.org:8443/x" << true << "example.org";
        QTest::newRow("mailto") << "mailto:me@example.org?subject=hi" << true << "";
        QTest::newRow("javascript") << "javascript:alert(1)" << false << "";
        QTest::newRow("file") << "file:///etc/passwd" << false << "";
        QTest::newRow("ftp") << "ftp://example.org/x" << false << "";
        QTest::newRow("data") << "data:text/html,<script>1</script>" << false << "";
        QTest::newRow("a local program") << "calc.exe" << false << "";
        QTest::newRow("no scheme") << "//example.org/x" << false << "";
        QTest::newRow("no host") << "http://" << false << "";
        QTest::newRow("empty") << "" << false << "";
        QTest::newRow("a space") << "http://example.org/a b" << false << "";
        QTest::newRow("a control character") << "http://example.org/\x01" << false << "";
        QTest::newRow("a new line") << "http://example.org/\nx" << false << "";
        QTest::newRow("an empty mail address") << "mailto:" << false << "";
    }

    void onlyWebAndMailAddressesMayBeOpened() {
        QFETCH(QString, uri);
        QFETCH(bool, allowed);
        QFETCH(QString, host);
        const LinkActions::UriCheck check = LinkActions::checkUri(uri);
        QCOMPARE(check.allowed, allowed);
        QCOMPARE(check.host, host);
        QCOMPARE(check.problem.isEmpty(), allowed);
    }

    void aLinkIsDescribedInWords() {
        const auto label = [](quint32 page) { return QStringLiteral("p%1").arg(page); };
        vellora::Link link;
        link.kind = vellora::LinkKind::GoTo;
        link.destination.page = 3;
        QCOMPARE(LinkActions::describe(link, label), QStringLiteral("Go to page p3"));
        link.kind = vellora::LinkKind::Uri;
        link.text = QStringLiteral("https://example.org/<b>");
        QCOMPARE(LinkActions::describe(link, label), link.text);
        link.text = QString(1000, QLatin1Char('a'));
        QCOMPARE(LinkActions::describe(link, label).size(), LinkActions::kMaxToolTipChars + 1);
        link.kind = vellora::LinkKind::Named;
        link.named = vellora::NamedKind::LastPage;
        QCOMPARE(LinkActions::describe(link, label), QStringLiteral("Go to the last page"));
        link.kind = vellora::LinkKind::Inert;
        link.text = QStringLiteral("Launch");
        QCOMPARE(LinkActions::describe(link, label),
                 QStringLiteral("This link would run an action Vellora never runs (Launch)."));
        link.kind = vellora::LinkKind::Unresolved;
        QVERIFY(LinkActions::describe(link, label).isEmpty());
    }

    // ---- the question ----

    void theQuestionShowsTheWholeAddressAndDefaultsToNotOpening() {
        const QString uri =
            QStringLiteral("https://example.org/") + QString(2000, QLatin1Char('a'));
        vellora::UriConfirmDialog dialog(uri, QStringLiteral("exa&mple.org"));
        auto* address = dialog.findChild<QPlainTextEdit*>();
        QVERIFY(address != nullptr);
        QCOMPARE(address->toPlainText(), uri);
        QVERIFY(address->isReadOnly());
        auto* trust = dialog.findChild<QCheckBox*>();
        QVERIFY(trust != nullptr);
        QVERIFY(!trust->isChecked());
        QVERIFY(!dialog.trustHost());
        // An ampersand in the host is not a mnemonic.
        QVERIFY2(trust->text().contains(QStringLiteral("exa&&mple.org")),
                 qPrintable(trust->text()));
        trust->setChecked(true);
        QVERIFY(dialog.trustHost());

        // A mail address has no host to trust.
        vellora::UriConfirmDialog mail(QStringLiteral("mailto:me@example.org"), QString());
        QVERIFY(mail.findChild<QCheckBox*>() == nullptr);
        QVERIFY(!mail.trustHost());
    }

    // ---- the pointer and the tool tip ----

    void theLinksAreWhereTheyAreOnThePage() {
        Fixture f;
        QCOMPARE(f.links()->linksOf(0).size(), 11);
        for (const Link which : {Goto, HttpsUri, Launch, Nowhere}) {
            QVERIFY2(f.links()->hitTest(f.centreOf(which)).link != nullptr,
                     qPrintable(QString::number(which)));
        }
        // Between two rows, left of the column and on another page: nothing.
        const QPoint between = f.centreOf(Goto) + QPoint(0, 22);
        QVERIFY(f.links()->hitTest(between).link == nullptr);
        QVERIFY(f.links()->hitTest(QPoint(1, 1)).link == nullptr);
        QCOMPARE(f.links()->hitTest(f.centreOf(Goto)).page, 0U);
    }

    void thePointerSaysWhatClickingWouldDo() {
        Fixture f;
        QCOMPARE(f.view().linkCursorAt(f.centreOf(Goto)), Qt::PointingHandCursor);
        QCOMPARE(f.view().linkCursorAt(f.centreOf(HttpsUri)), Qt::PointingHandCursor);
        QCOMPARE(f.view().linkCursorAt(f.centreOf(NextPage)), Qt::PointingHandCursor);
        // What Vellora never runs, and a link that goes nowhere, are not clickable.
        QCOMPARE(f.view().linkCursorAt(f.centreOf(Launch)), Qt::ForbiddenCursor);
        QCOMPARE(f.view().linkCursorAt(f.centreOf(JavaScriptAction)), Qt::ForbiddenCursor);
        QCOMPARE(f.view().linkCursorAt(f.centreOf(Nowhere)), Qt::ArrowCursor);
        QCOMPARE(f.view().linkCursorAt(QPoint(1, 1)), Qt::ArrowCursor);

        // Moving the pointer sets the cursor the viewport shows.
        QTest::mouseMove(f.view().viewport(), f.centreOf(Goto));
        QCOMPARE(f.view().viewport()->cursor().shape(), Qt::PointingHandCursor);
        QTest::mouseMove(f.view().viewport(), f.centreOf(Launch));
        QCOMPARE(f.view().viewport()->cursor().shape(), Qt::ForbiddenCursor);
        QTest::mouseMove(f.view().viewport(), QPoint(1, 1));
        QCOMPARE(f.view().viewport()->cursor().shape(), Qt::ArrowCursor);
    }

    void theToolTipShowsTheTarget() {
        Fixture f;
        QCOMPARE(f.view().linkToolTipAt(f.centreOf(Goto)), QStringLiteral("Go to page 2"));
        QCOMPARE(f.view().linkToolTipAt(f.centreOf(HttpsUri)),
                 QStringLiteral("https://example.org/path?q=1"));
        QCOMPARE(f.view().linkToolTipAt(f.centreOf(NextPage)),
                 QStringLiteral("Go to the next page"));
        QCOMPARE(f.view().linkToolTipAt(f.centreOf(Launch)),
                 QStringLiteral("This link would run an action Vellora never runs (Launch)."));
        QVERIFY(f.view().linkToolTipAt(f.centreOf(Nowhere)).isEmpty());
        QVERIFY(f.view().linkToolTipAt(QPoint(1, 1)).isEmpty());
    }

    void linksStayWhereTheyAreInATurnedView() {
        Fixture f;
        f.controller()->setRotation(1);
        QTRY_VERIFY_WITH_TIMEOUT(!f.controller()->frame().pages.isEmpty(), kWaitMs);
        const vellora::PageDraw page = f.controller()->frame().pages.first();
        int found = 0;
        for (const vellora::Link& link : f.links()->linksOf(0)) {
            const QPoint centre = page.map(link.rect).center().toPoint();
            const auto hit = f.links()->hitTest(centre);
            QVERIFY(hit.link != nullptr);
            QCOMPARE(hit.link->rect, link.rect);
            ++found;
        }
        QCOMPARE(found, 11);
    }

    // ---- clicking ----

    void anInternalLinkJumpsAndIsRecordedInTheHistory() {
        Fixture f;
        f.click(Goto);
        QCOMPARE(f.controller()->currentPage(), 1U);
        QCOMPARE(f.tab->history().backCount(), 1);
        QVERIFY(f.tab->goBack());
        QCOMPARE(f.controller()->currentPage(), 0U);
        // Nothing was asked or opened.
        QVERIFY(f.asked.isEmpty() && f.opened.isEmpty() && f.notices.isEmpty());
    }

    void aNamedPageActionTurnsThePage() {
        Fixture f;
        f.click(NextPage);
        QCOMPARE(f.controller()->currentPage(), 1U);
        QCOMPARE(f.tab->history().backCount(), 1);
    }

    void anAddressIsOpenedOnlyAfterTheQuestionIsAnsweredYes() {
        Fixture f;
        f.answer = {false, false};
        f.click(HttpsUri);
        QCOMPARE(f.asked, QStringList{QStringLiteral("https://example.org/path?q=1")});
        QCOMPARE(f.askedHosts, QStringList{QStringLiteral("example.org")});
        QVERIFY(f.opened.isEmpty());

        f.answer = {true, false};
        f.click(HttpsUri);
        QCOMPARE(f.asked.size(), 2);
        QCOMPARE(f.opened.size(), 1);
        QCOMPARE(f.opened.first().toString(), QStringLiteral("https://example.org/path?q=1"));
        // The reader is asked every time, and the view did not move.
        QCOMPARE(f.controller()->currentPage(), 0U);
        QCOMPARE(f.tab->history().backCount(), 0);
        QVERIFY(f.notices.isEmpty());
    }

    void notAskingAgainIsPerHostAndPerDocument() {
        Fixture f;
        f.answer = {true, true};
        f.click(TrustedA);
        QCOMPARE(f.asked.size(), 1);
        QVERIFY(f.tab->isHostTrusted(QStringLiteral("trusted.example")));
        // The same host: opened without a question. Another host: asked.
        f.click(TrustedB);
        QCOMPARE(f.asked.size(), 1);
        QCOMPARE(f.opened.size(), 2);
        f.answer = {true, false}; // opened this once, not trusted
        f.click(OtherHost);
        QCOMPARE(f.asked.size(), 2);
        QCOMPARE(f.askedHosts.last(), QStringLiteral("other.example"));
        QVERIFY(!f.tab->isHostTrusted(QStringLiteral("other.example")));

        // A cancelled question trusts nothing, whatever the box said.
        Fixture g;
        g.answer = {false, true};
        g.click(TrustedA);
        QVERIFY(!g.tab->isHostTrusted(QStringLiteral("trusted.example")));
    }

    void aMailAddressIsAskedAboutEveryTime() {
        Fixture f;
        f.answer = {true, true}; // "trust" has no host to apply to
        f.click(Mailto);
        f.click(Mailto);
        QCOMPARE(f.asked.size(), 2);
        QCOMPARE(f.askedHosts.first(), QString());
        QCOMPARE(f.opened.size(), 2);
        QCOMPARE(f.opened.first().scheme(), QStringLiteral("mailto"));
    }

    void anAddressOfAnotherKindIsShownAndRefusedWithoutAsking() {
        Fixture f;
        f.click(JavascriptUri);
        QVERIFY(f.asked.isEmpty());
        QVERIFY(f.opened.isEmpty());
        QCOMPARE(f.notices.size(), 1);
        QVERIFY2(f.notices.first().contains(QStringLiteral("javascript:alert(1)")),
                 qPrintable(f.notices.first()));
        QVERIFY2(f.notices.first().contains(QStringLiteral("not opened")),
                 qPrintable(f.notices.first()));
    }

    void anActionVelloraNeverRunsIsDescribedAndLeftAlone() {
        Fixture f;
        QSignalSpy messages(f.tab, &DocumentTab::message);
        f.click(Launch);
        f.click(JavaScriptAction);
        QCOMPARE(
            f.notices,
            (QStringList{
                QStringLiteral("This link would run an action Vellora never runs (Launch)."),
                QStringLiteral("This link would run an action Vellora never runs (JavaScript).")}));
        QVERIFY(f.asked.isEmpty() && f.opened.isEmpty());
        QCOMPARE(f.controller()->currentPage(), 0U);
        QCOMPARE(f.tab->history().backCount(), 0);
        QVERIFY(messages.size() >= 2); // also in the status bar
    }

    void aLinkThatGoesNowhereDoesNothing() {
        Fixture f;
        f.click(Nowhere);
        QVERIFY(f.asked.isEmpty() && f.opened.isEmpty() && f.notices.isEmpty());
        QCOMPARE(f.tab->history().backCount(), 0);
    }

    void aDragThatEndsElsewhereIsNotAClick() {
        Fixture f;
        QTest::mousePress(f.view().viewport(), Qt::LeftButton, {}, f.centreOf(Goto));
        QTest::mouseRelease(f.view().viewport(), Qt::LeftButton, {}, f.centreOf(HttpsUri));
        // Pressed on one link and released on another, or off any: neither is activated.
        QTest::mousePress(f.view().viewport(), Qt::LeftButton, {}, f.centreOf(Goto));
        QTest::mouseRelease(f.view().viewport(), Qt::LeftButton, {}, QPoint(1, 1));
        QCOMPARE(f.controller()->currentPage(), 0U);
        QVERIFY(f.asked.isEmpty() && f.opened.isEmpty());
        QCOMPARE(f.tab->history().backCount(), 0);
    }

    void aDocumentWithoutLinksHasNone() {
        vellora::MainWindow window;
        window.resize(900, 700);
        window.show();
        QSignalSpy opened(&window.session(), &vellora::EngineSession::opened);
        QVERIFY(window.openDocument(QStringLiteral(VELLORA_GOLDEN_PDF)));
        QVERIFY(opened.wait(kWaitMs));
        vellora::LinkLayer* links = window.canvas().links();
        QTRY_VERIFY_WITH_TIMEOUT(links->isComplete(0), kWaitMs);
        QVERIFY(links->linksOf(0).isEmpty());
        QCOMPARE(window.canvas().linkCursorAt(QPoint(50, 50)), Qt::ArrowCursor);
    }
};

VELLORA_TEST_MAIN(TstLinks)

#include "tst_links.moc"
