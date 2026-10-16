// M1 task 16b: printing. The page ranges and the placement of a page on the paper are tested as
// pure functions; the job, the permission check and the options dialog with a real engine. The
// printer is a `QPrinter` in PDF output mode (test-only: the application itself never offers it),
// and its output is read back by a second engine and compared with the source pages.
#include "MainWindow.h"
#include "VelloraTestMain.h"
#include "bridge/EngineSession.h"
#include "print/PageRange.h"
#include "print/PrintJob.h"
#include "print/PrintLayout.h"
#include "print/PrintOptionsDialog.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDir>
#include <QFile>
#include <QImage>
#include <QLabel>
#include <QLineEdit>
#include <QPageLayout>
#include <QPageSize>
#include <QPainter>
#include <QPrinter>
#include <QRadioButton>
#include <QSignalSpy>
#include <QTemporaryDir>
#include <QTest>
#include <cmath>
#include <cstdlib>
#include <memory>

using vellora::DocumentTab;
using vellora::EngineSession;
using vellora::PageParity;
using vellora::PrintContext;
using vellora::PrintJob;
using vellora::PrintOptions;
using vellora::PrintRequest;
using vellora::PrintScale;

namespace {

constexpr int kWaitMs = 60'000;

// ---- documents ----

// Eight pages labelled i, ii, 1, 2, 3, A-1, A-2, A-3, each 200 x 100 points.
QByteArray labelledPdf() {
    constexpr int kPages = 8;
    QByteArray out("%PDF-1.7\n");
    QList<qsizetype> offsets(3 + kPages, 0);
    const auto object = [&](int number, const QByteArray& body) {
        offsets[number] = out.size();
        out += QByteArray::number(number) + " 0 obj\n" + body + "\nendobj\n";
    };
    QByteArray kids;
    for (int i = 0; i < kPages; ++i) {
        kids += QByteArray::number(3 + i) + " 0 R ";
    }
    object(1, "<< /Type /Catalog /Pages 2 0 R /PageLabels << /Nums [0 << /S /r >> 2 << /S /D >> 5 "
              "<< /S /D /P (A-) >>] >> >>");
    object(2, "<< /Type /Pages /Kids [" + kids + "] /Count " + QByteArray::number(kPages) +
                  " /MediaBox [0 0 200 100] >>");
    for (int i = 0; i < kPages; ++i) {
        object(3 + i, "<< /Type /Page /Parent 2 0 R >>");
    }
    const int size = 3 + kPages;
    const qsizetype xref = out.size();
    out += "xref\n0 " + QByteArray::number(size) + "\n0000000000 65535 f \n";
    for (int number = 1; number < size; ++number) {
        out += QByteArray::number(offsets[number]).rightJustified(10, '0') + " 00000 n \n";
    }
    out += "trailer\n<< /Size " + QByteArray::number(size) + " /Root 1 0 R >>\nstartxref\n" +
           QByteArray::number(xref) + "\n%%EOF\n";
    return out;
}

// A page of `width` x `height` points with a red block in the corner, for the placement tests.
QByteArray onePagePdf(int width, int height, int pages = 1) {
    QByteArray out("%PDF-1.7\n");
    QList<qsizetype> offsets(3 + 2 * pages, 0);
    const auto object = [&](int number, const QByteArray& body) {
        offsets[number] = out.size();
        out += QByteArray::number(number) + " 0 obj\n" + body + "\nendobj\n";
    };
    QByteArray kids;
    for (int i = 0; i < pages; ++i) {
        kids += QByteArray::number(3 + 2 * i) + " 0 R ";
    }
    object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    object(2, "<< /Type /Pages /Kids [" + kids + "] /Count " + QByteArray::number(pages) +
                  " /MediaBox [0 0 " + QByteArray::number(width) + " " +
                  QByteArray::number(height) + "] >>");
    const QByteArray content = "1 0 0 rg 0 " + QByteArray::number(height - 20) + " 20 20 re f";
    for (int i = 0; i < pages; ++i) {
        object(3 + 2 * i, "<< /Type /Page /Parent 2 0 R /Contents " +
                              QByteArray::number(4 + 2 * i) + " 0 R >>");
        object(4 + 2 * i, "<< /Length " + QByteArray::number(content.size()) + " >>\nstream\n" +
                              content + "\nendstream");
    }
    const int size = 3 + 2 * pages;
    const qsizetype xref = out.size();
    out += "xref\n0 " + QByteArray::number(size) + "\n0000000000 65535 f \n";
    for (int number = 1; number < size; ++number) {
        out += QByteArray::number(offsets[number]).rightJustified(10, '0') + " 00000 n \n";
    }
    out += "trailer\n<< /Size " + QByteArray::number(size) + " /Root 1 0 R >>\nstartxref\n" +
           QByteArray::number(xref) + "\n%%EOF\n";
    return out;
}

QString writePdf(const QTemporaryDir& dir, const QString& name, const QByteArray& bytes) {
    const QString path = dir.filePath(name);
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly)) {
        qFatal("cannot write the test document");
    }
    file.write(bytes);
    file.close();
    return path;
}

// ---- reading pages back ----

bool openSession(EngineSession& session, const QString& path) {
    QSignalSpy opened(&session, &EngineSession::opened);
    return session.open(path).isEmpty() && opened.wait(kWaitMs);
}

// The page rendered by the engine at `scale`, as it prints (one band if it fits a slot).
QImage rasterise(EngineSession& session, quint32 page, float scale, bool grayscale = false) {
    const QSizeF size = session.pageSize(page);
    const auto width = static_cast<quint32>(std::ceil(size.width() * static_cast<double>(scale)));
    const auto height = static_cast<quint32>(std::ceil(size.height() * static_cast<double>(scale)));
    if (static_cast<quint64>(width) * height * 4 > session.slotBytes()) {
        qFatal("the page does not fit a slot");
    }
    bool ready = false;
    quint64 id = 0;
    const auto connection = QObject::connect(&session, &EngineSession::tileReady,
                                             [&](quint64 request) { ready |= request == id; });
    id = session.requestPrintBand(page, scale, 0, 0, width, height, grayscale);
    QImage image;
    if (id != 0 && QTest::qWaitFor([&] { return ready; }, kWaitMs)) {
        session.readPrintBand(id, width, height, image);
    }
    QObject::disconnect(connection);
    return image;
}

struct Difference {
    double mean = 0.0;
    double loud = 0.0;
};

// The measure of the render crate's golden test: the mean absolute difference of the channels, and
// the share of pixels where a channel differs by more than 32 levels.
Difference difference(const QImage& a, const QImage& b) {
    Difference out;
    if (a.size() != b.size()) {
        return {1e9, 1.0};
    }
    qint64 total = 0;
    qint64 loud = 0;
    for (int y = 0; y < a.height(); ++y) {
        const auto* pa = reinterpret_cast<const QRgb*>(a.constScanLine(y));
        const auto* pb = reinterpret_cast<const QRgb*>(b.constScanLine(y));
        for (int x = 0; x < a.width(); ++x) {
            const int dr = std::abs(qRed(pa[x]) - qRed(pb[x]));
            const int dg = std::abs(qGreen(pa[x]) - qGreen(pb[x]));
            const int db = std::abs(qBlue(pa[x]) - qBlue(pb[x]));
            total += dr + dg + db;
            loud += (dr > 32 || dg > 32 || db > 32) ? 1 : 0;
        }
    }
    const double pixels = static_cast<double>(a.width()) * a.height();
    out.mean = static_cast<double>(total) / (3.0 * pixels);
    out.loud = static_cast<double>(loud) / pixels;
    return out;
}

// The limits of the render crate's golden test.
constexpr double kMeanLimit = 0.5;
constexpr double kLoudLimit = 0.005;

// A printer that writes a PDF: the paper is `width` x `height` points with no margins, and a
// device pixel is 1/`dpi` of an inch.
std::shared_ptr<QPrinter> pdfPrinter(const QString& path, int width, int height, int dpi) {
    auto printer = std::make_shared<QPrinter>(QPrinter::HighResolution);
    printer->setOutputFormat(QPrinter::PdfFormat);
    printer->setOutputFileName(path);
    printer->setResolution(dpi);
    printer->setPageSize(QPageSize(QSizeF(width, height), QPageSize::Point));
    printer->setPageMargins(QMarginsF(0, 0, 0, 0), QPageLayout::Point);
    return printer;
}

QList<quint32> range(quint32 first, quint32 last) {
    QList<quint32> pages;
    for (quint32 page = first; page <= last; ++page) {
        pages.append(page);
    }
    return pages;
}

struct Fixture {
    QTemporaryDir dir;
    vellora::MainWindow window;
    DocumentTab* tab = nullptr;
    QStringList notices;
    int prompts = 0;
    PrintContext lastContext;

    explicit Fixture(const QString& path, bool copyToTemp = false) {
        window.resize(900, 700);
        window.show();
        tab = &window.currentTab();
        tab->setNotifier([this](const QString& text) { notices.append(text); });
        QString opened = path;
        if (copyToTemp) {
            opened = dir.filePath(QStringLiteral("doc.pdf"));
            QFile::copy(path, opened);
        }
        QSignalSpy openedSpy(&window.session(), &EngineSession::opened);
        if (!window.openDocument(opened) || !openedSpy.wait(kWaitMs)) {
            qFatal("could not open the test document");
        }
    }

    // Lets go of the printer the prompt holds, so that its file is closed once the caller has too.
    void forget() { tab->setPrintPrompt({}); }

    // Answers the next print dialog with `request` (or cancels, if it has no value).
    void answer(std::optional<PrintRequest> request) {
        tab->setPrintPrompt([this, request](const PrintContext& context) {
            ++prompts;
            lastContext = context;
            return request;
        });
    }
};

} // namespace

class TstPrint : public QObject {
    Q_OBJECT

private slots:
    // ---- page ranges ----

    void aRangeNamesPagesByNumberAndByLabel() {
        const auto resolve = [](const QString& text) -> std::optional<quint32> {
            const QStringList labels = {"i", "ii", "1", "2", "3", "A-1", "A-2", "A-3"};
            const qsizetype at = labels.indexOf(text);
            if (at >= 0) {
                return static_cast<quint32>(at);
            }
            return std::nullopt;
        };
        const auto pages = [&](const QString& text) {
            return vellora::parsePageRanges(text, resolve);
        };

        QCOMPARE(pages("1").pages, (QList<quint32>{2}));
        QCOMPARE(pages(" ii , 3 ").pages, (QList<quint32>{1, 4}));
        QCOMPARE(pages("ii-2").pages, (QList<quint32>{1, 2, 3}));
        // A hyphen inside a label is not a range; a range between such labels is.
        QCOMPARE(pages("A-2").pages, (QList<quint32>{6}));
        QCOMPARE(pages("A-1-A-3").pages, (QList<quint32>{5, 6, 7}));
        QCOMPARE(pages("3;i").pages, (QList<quint32>{4, 0}));
        // Each page once, in the order typed.
        QCOMPARE(pages("2-3, 3, 2").pages, (QList<quint32>{3, 4}));
        QCOMPARE(pages("1 – 3").pages, (QList<quint32>{2, 3, 4}));
        QVERIFY(pages("1-3").ok());
    }

    void aRangeThatIsNotUnderstoodSaysWhy() {
        const auto resolve = [](const QString& text) -> std::optional<quint32> {
            bool ok = false;
            const quint32 n = text.toUInt(&ok);
            if (ok && n >= 1 && n <= 10) {
                return n - 1;
            }
            return std::nullopt;
        };
        for (const char* bad : {"", "  ", ",", "0", "11", "abc", "3-x", "1-", "-3"}) {
            const auto result = vellora::parsePageRanges(QString::fromLatin1(bad), resolve);
            QVERIFY2(!result.ok(), bad);
            QVERIFY2(result.pages.isEmpty(), bad);
            QVERIFY2(!result.error.isEmpty(), bad);
        }
        const auto backwards = vellora::parsePageRanges("5-2", resolve);
        QVERIFY(!backwards.ok());
        QVERIFY2(backwards.error.contains("backwards"), qPrintable(backwards.error));
        // One bad piece spoils the whole text: nothing is printed on a guess.
        QVERIFY(!vellora::parsePageRanges("1-2, 99", resolve).ok());
    }

    // ---- placing a page on the paper ----

    void fitToPageFillsTheShorterSideAndCentres() {
        PrintOptions options;
        // A4 portrait at 72 dpi is 595 x 842 device pixels; a 200 x 100 page turned to fit.
        options.autoRotate = false;
        const auto fit = vellora::placePage({200, 100}, {595, 842}, 72.0, options);
        QVERIFY(!fit.rotated);
        QCOMPARE(fit.scale, 595.0 / 200.0);
        QCOMPARE(fit.size, QSizeF(595.0, 297.5));
        QCOMPARE(fit.origin, QPointF(0.0, (842.0 - 297.5) / 2.0));
        QCOMPARE(fit.renderScale, fit.scale);

        options.center = false;
        QCOMPARE(vellora::placePage({200, 100}, {595, 842}, 72.0, options).origin, QPointF());
    }

    void aPageIsTurnedToMatchThePaperWhenAsked() {
        PrintOptions options;
        const auto turned = vellora::placePage({200, 100}, {595, 842}, 72.0, options);
        QVERIFY(turned.rotated);
        // Shown as 100 x 200 points, so the paper's height is what limits it.
        QCOMPARE(turned.scale, 842.0 / 200.0);
        QCOMPARE(turned.size, QSizeF(100.0 * turned.scale, 842.0));
        // A square page, or one already the paper's way round, is left as it is.
        QVERIFY(!vellora::placePage({100, 100}, {595, 842}, 72.0, options).rotated);
        QVERIFY(!vellora::placePage({100, 200}, {595, 842}, 72.0, options).rotated);
        options.autoRotate = false;
        QVERIFY(!vellora::placePage({200, 100}, {595, 842}, 72.0, options).rotated);
    }

    void actualSizeKeepsPointsAndShrinkOversizedOnlyShrinks() {
        PrintOptions options;
        options.autoRotate = false;
        options.scale = PrintScale::ActualSize;
        // At 600 dpi a point is 600/72 device pixels, whatever the paper.
        auto actual = vellora::placePage({1000, 1000}, {4960, 7016}, 600.0, options);
        QCOMPARE(actual.scale, 600.0 / 72.0);
        QCOMPARE(actual.size, QSizeF(1000 * 600.0 / 72.0, 1000 * 600.0 / 72.0));
        // Larger than the paper: cut off (the origin goes negative when centred).
        actual = vellora::placePage({700, 700}, {4960, 7016}, 600.0, options);
        QVERIFY(actual.size.width() > 4960.0);
        QVERIFY(actual.origin.x() < 0.0);

        options.scale = PrintScale::ShrinkOversized;
        const auto small = vellora::placePage({100, 100}, {4960, 7016}, 600.0, options);
        QCOMPARE(small.scale, 600.0 / 72.0); // not enlarged
        const auto big = vellora::placePage({700, 700}, {4960, 7016}, 600.0, options);
        QVERIFY(big.scale < 600.0 / 72.0);
        QVERIFY(big.size.width() <= 4960.0 + 1e-6);
    }

    void aDocumentLimitedToLowResolutionRendersCoarserAndDrawsScaledUp() {
        PrintOptions options;
        const auto full = vellora::placePage({200, 100}, {4960, 7016}, 600.0, options);
        QCOMPARE(full.renderScale, full.scale);
        options.maxDpi = vellora::kLowResolutionDpi;
        const auto low = vellora::placePage({200, 100}, {4960, 7016}, 600.0, options);
        // Drawn the same size on the paper, from a render a quarter as dense.
        QCOMPARE(low.scale, full.scale);
        QCOMPARE(low.size, full.size);
        QCOMPARE(low.renderScale, full.scale * vellora::kLowResolutionDpi / 600.0);
        // A printer that is coarser than the limit is not held back.
        const auto coarse = vellora::placePage({200, 100}, {600, 800}, 72.0, options);
        QCOMPARE(coarse.renderScale, coarse.scale);
    }

    void oddAndEvenAreCountedFromOneAndCopiesFollowTheCollateSetting() {
        const QList<quint32> pages = range(0, 4);
        QCOMPARE(vellora::filterParity(pages, PageParity::Odd), (QList<quint32>{0, 2, 4}));
        QCOMPARE(vellora::filterParity(pages, PageParity::Even), (QList<quint32>{1, 3}));
        QCOMPARE(vellora::filterParity(pages, PageParity::All), pages);

        const QList<quint32> two{4, 7};
        QCOMPARE(vellora::sheetOrder(two, 2, true, false), (QList<quint32>{4, 7, 4, 7}));
        QCOMPARE(vellora::sheetOrder(two, 2, false, false), (QList<quint32>{4, 4, 7, 7}));
        QCOMPARE(vellora::sheetOrder(two, 1, true, true), (QList<quint32>{7, 4}));
        QCOMPARE(vellora::sheetOrder(two, 0, true, false), two); // below one is one
    }

    void bandsAreAsWideAsThePageUpToTheLimitAndAsHighAsASlotAllows() {
        constexpr quint32 kSlot = 1U << 20;
        // A small page is one band.
        QCOMPARE(PrintJob::bandSize(440, 280, kSlot), QSize(440, 280));
        // A letter page at 600 dpi: 4,950 wide is cut at the side limit, then 64 rows fit a slot.
        QCOMPARE(PrintJob::bandSize(4950, 6600, kSlot), QSize(4096, 64));
        // Never more than a slot, never empty.
        for (const quint32 w : {1U, 17U, 4096U, 10000U}) {
            for (const quint32 h : {1U, 90U, 7000U}) {
                const QSize band = PrintJob::bandSize(w, h, kSlot);
                QVERIFY(band.width() >= 1 && band.height() >= 1);
                QVERIFY(static_cast<quint64>(band.width()) * band.height() * 4 <= kSlot);
                QVERIFY(band.width() <= 4096 && band.height() <= 4096);
            }
        }
    }

    // ---- the dialog ----

    void theDialogReadsThePagesTheReaderChose() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, "labels.pdf", labelledPdf()));
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->hasPageLabels(), kWaitMs);
        f.window.currentTab().canvas().controller()->goToPage(4);
        f.answer(std::nullopt);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(f.prompts, 1, kWaitMs);
        PrintContext context = f.lastContext;
        context.currentPage = 4;
        QCOMPARE(context.pageCount, 8U);
        QCOMPARE(context.defaultRange, QStringLiteral("i-A-3"));
        QVERIFY(!context.lowResolutionOnly);
        // The label wins over the number: "2" is the fourth page here.
        QCOMPARE(context.resolve("2"), std::optional<quint32>(3));
        QCOMPARE(context.resolve("7"), std::optional<quint32>(6)); // no label "7": the number
        QVERIFY(!context.resolve("9").has_value());

        vellora::PrintOptionsDialog dialog(context);
        QCOMPARE(dialog.pages().size(), 8);
        dialog.currentPage()->setChecked(true);
        QCOMPARE(dialog.pages(), (QList<quint32>{4}));
        dialog.pageRange()->setChecked(true);
        QVERIFY(dialog.rangeField()->isEnabled());
        dialog.rangeField()->setText("ii-2, A-1-A-2");
        QCOMPARE(dialog.pages(), (QList<quint32>{1, 2, 3, 5, 6}));
        dialog.parity()->setCurrentIndex(1); // odd page numbers: 1st, 3rd, ...
        QCOMPARE(dialog.pages(), (QList<quint32>{2, 6}));
        dialog.parity()->setCurrentIndex(2);
        QCOMPARE(dialog.pages(), (QList<quint32>{1, 3, 5}));

        dialog.scale()->setCurrentIndex(1);
        dialog.autoRotate()->setChecked(false);
        dialog.center()->setChecked(false);
        dialog.grayscale()->setChecked(true);
        const PrintOptions options = dialog.options();
        QCOMPARE(options.scale, PrintScale::ActualSize);
        QVERIFY(!options.autoRotate && !options.center && options.grayscale);
    }

    void theDialogStaysOpenAndSaysWhyWhenTheRangeIsWrong() {
        PrintContext context;
        context.pageCount = 3;
        context.resolve = [](const QString& text) -> std::optional<quint32> {
            bool ok = false;
            const quint32 n = text.toUInt(&ok);
            return ok && n >= 1 && n <= 3 ? std::optional<quint32>(n - 1) : std::nullopt;
        };
        vellora::PrintOptionsDialog dialog(context);
        dialog.show();
        dialog.pageRange()->setChecked(true);
        dialog.rangeField()->setText("2-9");
        dialog.accept();
        QCOMPARE(dialog.result(), static_cast<int>(QDialog::Rejected));
        QVERIFY(dialog.isVisible());
        QVERIFY(dialog.problem()->isVisible());
        QVERIFY(dialog.problem()->text().contains("9"));
        dialog.rangeField()->setText("2-3");
        dialog.accept();
        QCOMPARE(dialog.result(), static_cast<int>(QDialog::Accepted));
        QCOMPARE(dialog.pages(), (QList<quint32>{1, 2}));
    }

    void theDialogOffersNoWayToWriteAPdf() {
        // Vellora's own writer is the only code that writes PDF; the dialogs offer a printer, not
        // a file. The options dialog has no file field, and the system dialog is opened without
        // its print-to-file option (see `promptForPrint`).
        PrintContext context;
        context.pageCount = 1;
        vellora::PrintOptionsDialog dialog(context);
        QVERIFY(dialog.findChildren<QLineEdit*>().size() == 1); // the page range only
    }

    // ---- what the document allows ----

    void aDocumentThatMayNotBePrintedSaysSoAndIsNotPrinted() {
        Fixture f(QDir(QStringLiteral(VELLORA_ENCRYPTION_DIR))
                      .filePath(QStringLiteral("r6-aes-256-no-print.pdf")));
        f.answer(std::nullopt);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(f.notices.size(), 1, kWaitMs);
        QVERIFY2(f.notices.first().contains("may not be printed"), qPrintable(f.notices.first()));
        QCOMPARE(f.prompts, 0);
        QVERIFY(f.tab->printJob() == nullptr);
        // The preview is a way of printing too.
        f.tab->printPreview();
        QTRY_COMPARE_WITH_TIMEOUT(f.notices.size(), 2, kWaitMs);
        QCOMPARE(f.prompts, 0);
    }

    void aDocumentThatAllowsLowResolutionPrintingIsHeldToIt() {
        Fixture f(QDir(QStringLiteral(VELLORA_ENCRYPTION_DIR))
                      .filePath(QStringLiteral("r3-rc4-128-low-res-print.pdf")));
        QTemporaryDir out;
        PrintRequest request;
        request.printer = pdfPrinter(out.filePath("low.pdf"), 400, 400, 600);
        request.sheets = {0};
        request.options.maxDpi = 0; // the prompt does not ask for a limit; the document does
        f.answer(request);
        int maxDpi = -1;
        QSignalSpy started(f.tab, &DocumentTab::printStarted);
        connect(f.tab, &DocumentTab::printStarted, this,
                [&](PrintJob* job) { maxDpi = job->options().maxDpi; });
        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(f.prompts, 1, kWaitMs);
        QVERIFY(f.lastContext.lowResolutionOnly);
        QVERIFY2(f.notices.value(0).contains("low resolution"), qPrintable(f.notices.value(0)));
        QCOMPARE(started.size(), 1);
        QCOMPARE(maxDpi, vellora::kLowResolutionDpi);
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
    }

    void aDocumentThatAllowsEverythingIsNotHeldBack() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, "plain.pdf", onePagePdf(200, 100)));
        f.answer(std::nullopt);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(f.prompts, 1, kWaitMs);
        QVERIFY(!f.lastContext.lowResolutionOnly);
        QVERIFY(f.notices.isEmpty());
    }

    // ---- printing ----

    void threePagesPrintedToAPdfMatchTheSource() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QCOMPARE(f.tab->session().pageCount(), 3U);
        QTemporaryDir out;
        const QString path = out.filePath("printed.pdf");
        // 144 dpi is two device pixels to the point; the paper is as large as the largest page and
        // pages are left at actual size in the top left corner, so that every printed pixel is
        // compared with a source pixel at the same scale.
        PrintRequest request;
        request.printer = pdfPrinter(path, 220, 160, 144);
        request.sheets = range(0, 2);
        request.options.scale = PrintScale::ActualSize;
        request.options.autoRotate = false;
        request.options.center = false;
        f.answer(request);

        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
        QVERIFY(f.tab->printJob() == nullptr);
        f.forget();
        request.printer.reset(); // closes the file

        EngineSession printed;
        QVERIFY(openSession(printed, path));
        QCOMPARE(printed.pageCount(), 3U);
        QStringList report;
        bool allWithin = true;
        for (quint32 page = 0; page < 3; ++page) {
            const QImage source = rasterise(f.tab->session(), page, 2.0F);
            const QImage output = rasterise(printed, page, 2.0F);
            QVERIFY2(!source.isNull() && !output.isNull(), "a page could not be rendered");
            QCOMPARE(output.size(), QSize(440, 320));
            // The page at actual size in the top left corner of white paper.
            QImage expected(output.size(), QImage::Format_RGB32);
            expected.fill(Qt::white);
            QPainter(&expected).drawImage(0, 0, source);
            const Difference d = difference(output, expected);
            // If the page is off, say where it went: the shift that fits best.
            QString where = QStringLiteral("best shift (0, 0)");
            double best = d.mean;
            for (int dy = -2; dy <= 2; ++dy) {
                for (int dx = -2; dx <= 2; ++dx) {
                    QImage moved(output.size(), QImage::Format_RGB32);
                    moved.fill(Qt::white);
                    QPainter(&moved).drawImage(dx, dy, output);
                    const double mean = difference(moved, expected).mean;
                    if (mean < best) {
                        best = mean;
                        where =
                            QStringLiteral("best shift (%1, %2) mean %3").arg(dx).arg(dy).arg(mean);
                    }
                }
            }
            report.append(
                QStringLiteral("page %1: mean difference %2, %3 of the pixels are off, %4")
                    .arg(page + 1)
                    .arg(d.mean)
                    .arg(d.loud)
                    .arg(where));
            allWithin &= d.mean <= kMeanLimit && d.loud <= kLoudLimit;
            // The comparison can tell pages apart: another page of the source is far off.
            const QImage other = rasterise(f.tab->session(), (page + 1) % 3, 2.0F);
            QImage otherPaper(output.size(), QImage::Format_RGB32);
            otherPaper.fill(Qt::white);
            QPainter(&otherPaper).drawImage(0, 0, other);
            QVERIFY2(difference(output, otherPaper).mean > 4.0 * kMeanLimit,
                     "a different page is not told apart");
        }
        QVERIFY2(allWithin, qPrintable(report.join(QStringLiteral("; "))));
    }

    void aPageIsTurnedFittedAndCentredOnThePaper() {
        QTemporaryDir dir;
        // 200 x 100 points with a red block at the top left, on portrait paper of 100 x 300.
        Fixture f(writePdf(dir, "wide.pdf", onePagePdf(200, 100)));
        QTemporaryDir out;
        const QString path = out.filePath("turned.pdf");
        PrintRequest request;
        request.printer = pdfPrinter(path, 100, 300, 72);
        request.sheets = {0};
        f.answer(request); // fit to page, turn, centre: the defaults
        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
        f.forget();
        request.printer.reset();

        EngineSession printed;
        QVERIFY(openSession(printed, path));
        const QImage page = rasterise(printed, 0, 1.0F);
        QCOMPARE(page.size(), QSize(100, 300));
        const auto isRed = [&](int x, int y) {
            const QColor c(page.pixel(x, y));
            return c.red() > 200 && c.green() < 60 && c.blue() < 60;
        };
        const auto isWhite = [&](int x, int y) {
            const QColor c(page.pixel(x, y));
            return c.red() > 240 && c.green() > 240 && c.blue() > 240;
        };
        // Turned a quarter turn clockwise, the page fills the paper's width (100 x 200) and is
        // centred: 50 points of white above and below. The block that was at the page's top left
        // is at the top right of the turned page.
        QVERIFY2(isRed(90, 60), "the block is at the top right of the turned page");
        QVERIFY2(isWhite(10, 60), "and nothing is at its top left");
        QVERIFY2(isWhite(50, 20), "white above the page");
        QVERIFY2(isWhite(50, 280), "white below the page");
    }

    void grayscaleBandsPrintInGrey() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, "red.pdf", onePagePdf(200, 100)));
        QTemporaryDir out;
        const QString path = out.filePath("grey.pdf");
        PrintRequest request;
        request.printer = pdfPrinter(path, 200, 100, 72);
        request.sheets = {0};
        request.options.grayscale = true;
        f.answer(request);
        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        f.forget();
        request.printer.reset();

        EngineSession printed;
        QVERIFY(openSession(printed, path));
        const QImage page = rasterise(printed, 0, 1.0F);
        const QColor block(page.pixel(10, 10));
        QVERIFY2(std::abs(block.red() - block.green()) < 8 &&
                     std::abs(block.green() - block.blue()) < 8,
                 "the red block is grey");
        QVERIFY2(block.red() < 200, "and it is not white");
    }

    void aLargePageIsPrintedInSeveralBandsAndProgressRunsToTheEnd() {
        QTemporaryDir dir;
        Fixture f(writePdf(dir, "two.pdf", onePagePdf(200, 100, 2)));
        QTemporaryDir out;
        // 1200 dpi: a page of 200 x 100 points is 3,333 x 1,667 pixels, about six slots.
        auto printer = pdfPrinter(out.filePath("bands.pdf"), 200, 100, 1200);
        PrintOptions options;
        PrintJob job(f.tab->session(), *printer, range(0, 1), options);
        int last = -1;
        int maximum = 0;
        bool monotone = true;
        connect(&job, &PrintJob::progress, this, [&](int value, int max) {
            monotone &= value >= last;
            last = value;
            maximum = max;
        });
        QSignalSpy finished(&job, &PrintJob::finished);
        QVERIFY(job.start().isEmpty());
        QVERIFY(job.isRunning());
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
        QVERIFY(monotone);
        QCOMPARE(last, maximum);
        QCOMPARE(maximum, 2000);
        // Each page is more than one band, and none is larger than a slot.
        QVERIFY2(job.bandsRequested() > 2, qPrintable(QString::number(job.bandsRequested())));
        QCOMPARE(job.sheetsDone(), 2);
    }

    void cancellingStopsAskingTheEngineAndFreesItsSlots() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QTemporaryDir out;
        // A4 at 1200 dpi: each page is hundreds of bands, far more than finish in this test.
        auto printer = pdfPrinter(out.filePath("cancelled.pdf"), 595, 842, 1200);
        PrintJob job(f.tab->session(), *printer, range(0, 2), PrintOptions{});
        QSignalSpy finished(&job, &PrintJob::finished);
        // Cancels as soon as the first band is on the page.
        connect(&job, &PrintJob::progress, &job, [&](int value, int) {
            if (value > 0) {
                job.cancel();
            }
        });
        QVERIFY(job.start().isEmpty());
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Cancelled);
        QVERIFY(!job.isRunning());
        // Nothing more is asked for after the cancel, and no more is drawn: only the bands that
        // were in flight (at most the print slots) were ever asked for.
        const int asked = job.bandsRequested();
        QTest::qWait(300);
        QCOMPARE(job.bandsRequested(), asked);
        QCOMPARE(finished.size(), 1);
        QVERIFY2(asked <= 8, qPrintable(QStringLiteral("%1 bands were asked for").arg(asked)));
        QCOMPARE(job.sheetsDone(), 0);

        // The engine took the withdrawn bands back: every print slot is free again, so a new job
        // gets all of its bands through.
        auto again = pdfPrinter(out.filePath("again.pdf"), 200, 100, 72);
        PrintJob second(f.tab->session(), *again, {0}, PrintOptions{});
        QSignalSpy secondDone(&second, &PrintJob::finished);
        QVERIFY(second.start().isEmpty());
        QTRY_COMPARE_WITH_TIMEOUT(secondDone.size(), 1, kWaitMs);
        QCOMPARE(secondDone.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
    }

    void cancellingFromTheTabEndsThePrintAndReportsIt() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QTemporaryDir out;
        PrintRequest request;
        request.printer = pdfPrinter(out.filePath("tab.pdf"), 595, 842, 1200);
        request.sheets = range(0, 2);
        f.answer(request);
        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->printJob() != nullptr, kWaitMs);
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->printJob()->bandsRequested() > 0, kWaitMs);
        // A second print while one runs is refused, politely.
        f.tab->print();
        QCOMPARE(f.notices.size(), 1);
        f.tab->printJob()->cancel();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Cancelled);
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->printJob() == nullptr, kWaitMs);
        // Printing works again afterwards.
        request.printer = pdfPrinter(out.filePath("tab2.pdf"), 200, 100, 72);
        request.sheets = {0};
        f.answer(request);
        f.tab->print();
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 2, kWaitMs);
        QCOMPARE(finished.last().at(0).value<PrintJob::Result>(), PrintJob::Result::Completed);
    }

    void openingAnotherDocumentEndsThePrintOfTheFirst() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QTemporaryDir out;
        PrintRequest request;
        request.printer = pdfPrinter(out.filePath("gone.pdf"), 595, 842, 1200);
        request.sheets = range(0, 2);
        f.answer(request);
        QSignalSpy finished(f.tab, &DocumentTab::printFinished);
        f.tab->print();
        QTRY_VERIFY_WITH_TIMEOUT(f.tab->printJob() != nullptr, kWaitMs);
        QTemporaryDir dir;
        f.tab->open(writePdf(dir, "other.pdf", onePagePdf(100, 100)));
        QTRY_COMPARE_WITH_TIMEOUT(finished.size(), 1, kWaitMs);
        QCOMPARE(finished.first().at(0).value<PrintJob::Result>(), PrintJob::Result::Cancelled);
    }

    void aPageTooLargeToPrintIsRefusedAndNotLoopedOver() {
        QTemporaryDir dir;
        // The largest page PDF viewers accept (200 inches) at actual size on a very fine printer:
        // hundreds of millions of pixels a side in bands, far past what is sensible to ask for.
        Fixture f(writePdf(dir, "huge.pdf", onePagePdf(14400, 14400)));
        QTemporaryDir out;
        auto printer = pdfPrinter(out.filePath("huge.pdf"), 100, 100, 4800);
        PrintOptions options;
        options.scale = PrintScale::ActualSize;
        PrintJob job(f.tab->session(), *printer, {0}, options);
        const QString error = job.start();
        QVERIFY2(error.contains("too large"), qPrintable(error));
        QVERIFY(!job.isRunning());
        QCOMPARE(job.bandsRequested(), 0);
    }

    void aJobWithoutPagesOrWithoutADocumentIsRefused() {
        EngineSession closed;
        auto printer = std::make_shared<QPrinter>(QPrinter::ScreenResolution);
        PrintJob none(closed, *printer, {}, PrintOptions{});
        QVERIFY(!none.start().isEmpty());
        PrintJob noDocument(closed, *printer, {0}, PrintOptions{});
        QVERIFY(!noDocument.start().isEmpty());
        QVERIFY(!noDocument.isRunning());
    }

    // ---- the preview ----

    void thePreviewDrawsThroughTheSamePathAtScreenResolution() {
        Fixture f(QStringLiteral(VELLORA_GOLDEN_PDF));
        QTemporaryDir out;
        const QString path = out.filePath("preview.pdf");
        auto printer = std::make_shared<QPrinter>(QPrinter::ScreenResolution);
        printer->setOutputFormat(QPrinter::PdfFormat);
        printer->setOutputFileName(path);
        printer->setPageSize(QPageSize(QSizeF(220, 160), QPageSize::Point));
        printer->setPageMargins(QMarginsF(0, 0, 0, 0), QPageLayout::Point);
        PrintRequest request;
        request.printer = printer;
        request.sheets = range(0, 1);
        QCOMPARE(f.tab->paintPreview(*printer, request), PrintJob::Result::Completed);
        printer.reset();
        f.forget();
        request.printer.reset();

        EngineSession printed;
        QVERIFY(openSession(printed, path));
        QCOMPARE(printed.pageCount(), 2U);
    }
};

VELLORA_TEST_MAIN(TstPrint)
#include "tst_print.moc"
