#include "print/PrintJob.h"

#include <QCoreApplication>
#include <QEventLoop>
#include <QPageLayout>
#include <QPrinter>
#include <algorithm>
#include <cmath>

namespace vellora {

namespace {

// The protocol's longest side of a tile.
constexpr quint32 kMaxBandSide = 4096;
// The largest side of a rendered page: the protocol's limit on where a tile starts (2^24 pixels).
constexpr double kMaxRenderSide = 16777216.0;
// More bands than a 200-inch page at 1,200 dpi needs (about 240,000 pixels a side, tens of
// thousands of bands); a page that needs more is refused.
constexpr quint64 kMaxBandsPerPage = 1'000'000;

QString tr(const char* text) {
    return QCoreApplication::translate("PrintJob", text);
}

} // namespace

QSize PrintJob::bandSize(quint32 width, quint32 height, quint32 slotBytes) {
    const quint32 pixels = std::max<quint32>(slotBytes / 4U, 1U);
    const quint32 w = std::clamp<quint32>(std::min(width, pixels), 1U, kMaxBandSide);
    const quint32 h = std::clamp<quint32>(std::min(height, pixels / w), 1U, kMaxBandSide);
    return {static_cast<int>(w), static_cast<int>(h)};
}

PrintJob::PrintJob(EngineSession& session, QPrinter& printer, QList<quint32> sheets,
                   PrintOptions options, QObject* parent)
    : QObject(parent), m_session(session), m_printer(printer), m_pages(std::move(sheets)),
      m_options(options) {
    connect(&m_session, &EngineSession::tileReady, this, &PrintJob::onTileReady);
    connect(&m_session, &EngineSession::requestFailed, this, &PrintJob::onRequestFailed);
    connect(&m_session, &EngineSession::engineCrashed, this, &PrintJob::onEngineCrashed);
    connect(&m_session, &EngineSession::engineRestarted, this, &PrintJob::onEngineRestarted);
    connect(&m_session, &EngineSession::failed, this, &PrintJob::onFailed);
}

PrintJob::~PrintJob() {
    if (m_running) {
        for (auto it = m_inflight.cbegin(); it != m_inflight.cend(); ++it) {
            m_session.cancel(it.key());
        }
        m_printer.abort();
        m_painter.end();
    }
}

QString PrintJob::start() {
    if (m_running) {
        return tr("The job is already running.");
    }
    if (m_pages.isEmpty()) {
        return tr("There are no pages to print.");
    }
    if (!m_session.isOpen()) {
        return tr("There is no open document to print.");
    }
    if (!m_painter.begin(&m_printer)) {
        return tr("The printer cannot be used.");
    }
    m_running = true;
    m_sheet = 0;
    m_bandsRequested = 0;
    m_message.clear();
    QString error;
    if (!beginSheet(error)) {
        // Refused outright: nothing was sent to the printer yet, so there is no job to abort.
        m_running = false;
        m_painter.end();
        m_message = error;
        return error;
    }
    emitProgress();
    pump();
    return {};
}

PrintJob::Result PrintJob::runToEnd() {
    QEventLoop loop;
    Result result = Result::Failed;
    connect(this, &PrintJob::finished, &loop, [&](Result r, const QString&) {
        result = r;
        loop.quit();
    });
    const QString error = start();
    if (!error.isEmpty()) {
        m_message = error;
        return Result::Failed;
    }
    if (m_running) {
        loop.exec();
    } else {
        // Over before it began (the first page could not be placed).
        return Result::Failed;
    }
    return result;
}

void PrintJob::cancel() {
    if (m_running) {
        finish(Result::Cancelled, tr("Printing was cancelled."));
    }
}

bool PrintJob::beginSheet(QString& error) {
    const quint32 page = m_pages.at(m_sheet);
    const QSizeF size = m_session.pageSize(page);
    if (!size.isValid() || size.isEmpty()) {
        error = tr("Page %1 has no size.").arg(page + 1);
        return false;
    }
    const int dpi = m_printer.resolution();
    const QSizeF printable = QSizeF(m_printer.pageLayout().paintRectPixels(dpi).size());
    if (printable.isEmpty() || dpi <= 0) {
        error = tr("The paper has no room to print on.");
        return false;
    }
    const Placement place = placePage(size, printable, dpi, m_options);
    m_renderScale = static_cast<float>(place.renderScale);
    // The size comes from the document, so it is checked before it is turned into an integer: a
    // page the protocol could not address (a tile origin is at most 2^24 pixels) or that would
    // take more bands than any real page is refused, not looped over.
    const double wantedWidth = std::ceil(size.width() * place.renderScale);
    const double wantedHeight = std::ceil(size.height() * place.renderScale);
    if (!(wantedWidth >= 1.0) || !(wantedHeight >= 1.0)) {
        error = tr("Page %1 is too small to print.").arg(page + 1);
        return false;
    }
    if (wantedWidth > kMaxRenderSide || wantedHeight > kMaxRenderSide) {
        error = tr("Page %1 is too large to print.").arg(page + 1);
        return false;
    }
    m_renderWidth = static_cast<quint32>(wantedWidth);
    m_renderHeight = static_cast<quint32>(wantedHeight);

    // Render pixels to device pixels: the factor between the two scales, then the quarter turn,
    // then the page's place on the paper.
    const double ratio = place.scale / place.renderScale;
    m_transform = QTransform();
    m_transform.translate(place.origin.x() + (place.rotated ? place.size.width() : 0.0),
                          place.origin.y());
    if (place.rotated) {
        m_transform.rotate(90.0);
    }
    m_transform.scale(ratio, ratio);
    m_clip = QRectF(QPointF(0.0, 0.0), printable);

    const QSize band = bandSize(m_renderWidth, m_renderHeight, m_session.slotBytes());
    const quint64 columns = (m_renderWidth + static_cast<quint32>(band.width()) - 1U) /
                            static_cast<quint32>(band.width());
    const quint64 rows = (m_renderHeight + static_cast<quint32>(band.height()) - 1U) /
                         static_cast<quint32>(band.height());
    if (columns * rows > kMaxBandsPerPage) {
        error = tr("Page %1 is too large to print.").arg(page + 1);
        return false;
    }
    m_queue.clear();
    m_inflight.clear();
    for (quint32 y = 0; y < m_renderHeight; y += static_cast<quint32>(band.height())) {
        for (quint32 x = 0; x < m_renderWidth; x += static_cast<quint32>(band.width())) {
            const QRect rect(static_cast<int>(x), static_cast<int>(y),
                             static_cast<int>(std::min<quint32>(static_cast<quint32>(band.width()),
                                                                m_renderWidth - x)),
                             static_cast<int>(std::min<quint32>(static_cast<quint32>(band.height()),
                                                                m_renderHeight - y)));
            // A band the paper does not show (a page cut off at actual size) is not rendered.
            if (m_transform.mapRect(QRectF(rect)).intersects(m_clip)) {
                m_queue.append(rect);
            }
        }
    }
    m_bandsInSheet = static_cast<quint32>(m_queue.size());
    m_bandsDrawn = 0;
    return true;
}

void PrintJob::pump() {
    if (!m_running || m_waitingForEngine) {
        return;
    }
    const quint32 page = m_pages.at(m_sheet);
    while (!m_queue.isEmpty()) {
        const QRect band = m_queue.first();
        const quint64 id = m_session.requestPrintBand(
            page, m_renderScale, static_cast<quint32>(band.x()), static_cast<quint32>(band.y()),
            static_cast<quint32>(band.width()), static_cast<quint32>(band.height()),
            m_options.grayscale);
        if (id == 0) {
            if (m_inflight.isEmpty()) {
                finish(Result::Failed, tr("The engine did not accept page %1.").arg(page + 1));
            }
            // Else every print slot is busy: a finished band frees one.
            return;
        }
        ++m_bandsRequested;
        m_inflight.insert(id, band);
        m_queue.removeFirst();
    }
    if (m_inflight.isEmpty()) {
        sheetFinished();
    }
}

void PrintJob::onTileReady(quint64 request) {
    const auto it = m_inflight.constFind(request);
    if (!m_running || it == m_inflight.cend()) {
        return;
    }
    const QRect band = it.value();
    m_inflight.erase(it);
    if (!m_session.readPrintBand(request, static_cast<quint32>(band.width()),
                                 static_cast<quint32>(band.height()), m_image)) {
        finish(Result::Failed,
               tr("A part of page %1 could not be read.").arg(m_pages.at(m_sheet) + 1));
        return;
    }
    drawBand(band, m_image);
    ++m_bandsDrawn;
    emitProgress();
    pump();
}

void PrintJob::drawBand(const QRect& band, const QImage& image) {
    m_painter.save();
    m_painter.setClipRect(m_clip);
    m_painter.setRenderHint(QPainter::SmoothPixmapTransform, true);
    m_painter.setTransform(m_transform, true);
    m_painter.drawImage(band.topLeft(), image);
    m_painter.restore();
}

void PrintJob::sheetFinished() {
    ++m_sheet;
    if (m_sheet >= m_pages.size()) {
        emitProgress();
        finish(Result::Completed, {});
        return;
    }
    if (!m_printer.newPage()) {
        finish(Result::Failed, tr("The printer did not take the next page."));
        return;
    }
    QString error;
    if (!beginSheet(error)) {
        finish(Result::Failed, error);
        return;
    }
    emitProgress();
    pump();
}

void PrintJob::emitProgress() {
    constexpr int kPerSheet = 1000;
    int within = 0;
    if (m_bandsInSheet > 0 && m_sheet < m_pages.size()) {
        within = static_cast<int>(static_cast<qint64>(m_bandsDrawn) * kPerSheet / m_bandsInSheet);
    }
    emit progress(m_sheet * kPerSheet + within, static_cast<int>(m_pages.size()) * kPerSheet);
}

void PrintJob::finish(Result result, const QString& message) {
    if (!m_running) {
        return;
    }
    m_running = false;
    for (auto it = m_inflight.cbegin(); it != m_inflight.cend(); ++it) {
        m_session.cancel(it.key());
    }
    m_inflight.clear();
    m_queue.clear();
    if (result != Result::Completed) {
        m_printer.abort();
    }
    m_painter.end();
    m_message = message;
    emit finished(result, message);
}

void PrintJob::onRequestFailed(quint64 request, const QString& message) {
    if (m_running && m_inflight.contains(request)) {
        finish(Result::Failed, message);
    }
}

void PrintJob::onEngineCrashed(const QString&, bool willRestart, const QList<quint64>& lost) {
    if (!m_running) {
        return;
    }
    if (!willRestart) {
        finish(Result::Failed, tr("The engine stopped."));
        return;
    }
    // What was in flight will never be answered; ask for it again once the engine is back.
    QList<QRect> again;
    for (const quint64 id : lost) {
        const auto it = m_inflight.find(id);
        if (it != m_inflight.end()) {
            again.append(it.value());
            m_inflight.erase(it);
        }
    }
    for (auto it = again.crbegin(); it != again.crend(); ++it) {
        m_queue.prepend(*it);
    }
    m_waitingForEngine = true;
}

void PrintJob::onEngineRestarted() {
    if (m_waitingForEngine) {
        m_waitingForEngine = false;
        pump();
    }
}

void PrintJob::onFailed(const QString& reason) {
    finish(Result::Failed, reason);
}

} // namespace vellora
