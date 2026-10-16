// The printing half of DocumentTab: asking the engine what the document allows, the prompts, the
// print job with its progress dialog, and the preview.
#include "DocumentTab.h"
#include "print/PrintOptionsDialog.h"

#include <QPrintDialog>
#include <QPrintPreviewDialog>
#include <QPrinter>
#include <QProgressDialog>
#include <algorithm>

namespace vellora {

void DocumentTab::print() {
    askToPrint(false);
}

void DocumentTab::printPreview() {
    askToPrint(true);
}

void DocumentTab::askToPrint(bool preview) {
    if (m_path.isEmpty() || !m_session.isOpen() || !m_opened) {
        return;
    }
    if (m_printJob != nullptr || m_permissionRequest != 0) {
        notify(tr("This document is already being printed."));
        return;
    }
    m_previewAsked = preview;
    m_permissionRequest = m_session.requestPermissions();
    if (m_permissionRequest == 0) {
        notify(tr("The document cannot be printed now: the engine is not available."));
    }
}

void DocumentTab::onPermissions(quint64 request, bool, bool owner, bool print,
                                bool printHighQuality) {
    if (request == 0 || request != m_permissionRequest) {
        return;
    }
    m_permissionRequest = 0;
    if (!print) {
        notify(tr("This document may not be printed: its author has not allowed it.\n\n"
                  "Opening it with its owner password lifts the restriction."));
        return;
    }
    const bool lowResolutionOnly = !printHighQuality && !owner;
    if (lowResolutionOnly) {
        notify(tr("This document allows printing at low resolution only (about %1 dpi), so the "
                  "print will look coarse.")
                   .arg(kLowResolutionDpi));
    }
    const bool preview = m_previewAsked;
    const PrintContext context = printContext(lowResolutionOnly, preview);
    std::optional<PrintRequest> chosen =
        m_printPrompt ? m_printPrompt(context) : promptForPrint(context);
    if (!chosen || !chosen->printer || chosen->sheets.isEmpty()) {
        return;
    }
    if (lowResolutionOnly) {
        // Whatever the prompt answered: the document's own limit.
        chosen->options.maxDpi = chosen->options.maxDpi > 0
                                     ? std::min(chosen->options.maxDpi, kLowResolutionDpi)
                                     : kLowResolutionDpi;
    }
    if (preview) {
        showPreview(*chosen);
    } else {
        startPrint(*chosen);
    }
}

PrintContext DocumentTab::printContext(bool lowResolutionOnly, bool preview) const {
    PrintContext context;
    context.pageCount = m_pageCount;
    context.currentPage = m_page;
    context.lowResolutionOnly = lowResolutionOnly;
    context.preview = preview;
    if (m_pageCount > 0) {
        context.defaultRange = pageLabel(0) + QLatin1Char('-') + pageLabel(m_pageCount - 1);
    }
    context.resolve = [this](const QString& text) -> std::optional<quint32> {
        // A label of the document first ("iv", "A-3"), else a page number.
        const quint32 labelled = std::min(m_pageCount, kMaxLabelledPages);
        for (quint32 page = 0; page < labelled; ++page) {
            if (pageLabel(page) == text) {
                return page;
            }
        }
        bool isNumber = false;
        const quint32 number = text.toUInt(&isNumber);
        if (isNumber && number >= 1 && number <= m_pageCount) {
            return number - 1;
        }
        return std::nullopt;
    };
    return context;
}

std::optional<PrintRequest> DocumentTab::promptForPrint(const PrintContext& context) {
    PrintOptionsDialog options(context, this);
    if (options.exec() != QDialog::Accepted) {
        return std::nullopt;
    }
    PrintRequest request;
    request.options = options.options();
    const QList<quint32> pages = options.pages();
    if (context.preview) {
        request.printer = std::make_shared<QPrinter>(QPrinter::ScreenResolution);
        request.sheets = pages;
        return request;
    }
    request.printer = std::make_shared<QPrinter>(QPrinter::HighResolution);
    // No "print to file": Vellora's own writer is the only thing that writes PDF, and a reader who
    // wants a PDF printer can choose the one the system provides.
    QPrintDialog dialog(request.printer.get(), this);
    dialog.setOptions(QAbstractPrintDialog::PrintCollateCopies |
                      QAbstractPrintDialog::PrintShowPageSize);
    if (dialog.exec() != QDialog::Accepted) {
        return std::nullopt;
    }
    // A printer that cannot make copies leaves them to the program.
    const int copies = request.printer->supportsMultipleCopies() ? 1 : request.printer->copyCount();
    request.sheets = sheetOrder(pages, copies, request.printer->collateCopies(),
                                request.printer->pageOrder() == QPrinter::LastPageFirst);
    return request;
}

void DocumentTab::startPrint(const PrintRequest& request) {
    m_printRequest = request;
    auto* job = new PrintJob(m_session, *m_printRequest.printer, request.sheets, request.options);
    m_printJob = job;
    m_printProgress =
        new QProgressDialog(tr("Printing %1…").arg(m_fileName), tr("Cancel"), 0, 1000, this);
    m_printProgress->setObjectName(QStringLiteral("printProgress"));
    m_printProgress->setWindowTitle(tr("Printing"));
    m_printProgress->setWindowModality(Qt::WindowModal);
    m_printProgress->setMinimumDuration(0);
    m_printProgress->setAutoClose(false);
    m_printProgress->setAutoReset(false);
    connect(job, &PrintJob::progress, m_printProgress, [this](int value, int maximum) {
        m_printProgress->setMaximum(maximum);
        m_printProgress->setValue(value);
    });
    connect(m_printProgress, &QProgressDialog::canceled, job, &PrintJob::cancel);
    connect(job, &PrintJob::finished, this, &DocumentTab::onPrintFinished);
    const QString error = job->start();
    if (!error.isEmpty()) {
        delete m_printProgress;
        m_printProgress = nullptr;
        delete job;
        m_printJob = nullptr;
        notify(tr("Printing failed: %1").arg(error));
        return;
    }
    if (m_printJob != nullptr) {
        m_printProgress->show();
    }
}

void DocumentTab::onPrintFinished(PrintJob::Result result, const QString& text) {
    // Deleted later: this runs inside one of the job's own signals.
    if (m_printProgress != nullptr) {
        m_printProgress->hide();
        m_printProgress->deleteLater();
        m_printProgress = nullptr;
    }
    if (m_printJob != nullptr) {
        m_printJob->deleteLater();
        m_printJob = nullptr;
    }
    switch (result) {
    case PrintJob::Result::Completed:
        emit message(tr("Printed."));
        break;
    case PrintJob::Result::Cancelled:
        emit message(tr("Printing was cancelled."));
        break;
    case PrintJob::Result::Failed:
        notify(tr("Printing failed: %1").arg(text));
        break;
    }
    emit printFinished(result, text);
}

PrintJob::Result DocumentTab::paintPreview(QPrinter& printer, const PrintRequest& request) {
    PrintJob job(m_session, printer, request.sheets, request.options);
    return job.runToEnd();
}

void DocumentTab::showPreview(const PrintRequest& request) {
    QPrintPreviewDialog preview(request.printer.get(), this);
    preview.setObjectName(QStringLiteral("printPreview"));
    connect(&preview, &QPrintPreviewDialog::paintRequested, this,
            [this, request](QPrinter* printer) { paintPreview(*printer, request); });
    preview.exec();
}

} // namespace vellora
