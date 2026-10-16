// Draws pages of the open document on a printer. The engine renders each page at the printer's
// resolution in bands (the pieces of the page that fit one shared-memory slot, so no buffer is
// bigger than a slot however large the page or the resolution), the shell draws every finished
// band with a `QPainter` and hands the page to the printer when its last band is in. The shell
// never parses the document: it asks, and draws what comes back.
//
// The job is asynchronous: it needs the event loop to receive the bands. `cancel` withdraws the
// bands still in the engine and stops asking for more.
#pragma once

#include "bridge/EngineSession.h"
#include "print/PrintLayout.h"

#include <QHash>
#include <QImage>
#include <QList>
#include <QObject>
#include <QPainter>
#include <QRect>
#include <QTransform>

class QPrinter;

namespace vellora {

class PrintJob : public QObject {
    Q_OBJECT

public:
    enum class Result { Completed, Cancelled, Failed };
    Q_ENUM(Result)

    // Prints `sheets` (zero-based page indices, in the order they are printed; see `sheetOrder`)
    // on `printer`, which must outlive the job. Neither the session nor the printer is owned.
    PrintJob(EngineSession& session, QPrinter& printer, QList<quint32> sheets, PrintOptions options,
             QObject* parent = nullptr);
    ~PrintJob() override;

    // Starts printing. An empty string if it started (`finished` follows later), else why not.
    QString start();
    // Starts and waits, running the event loop, until the job is over. For callers that must draw
    // before they return (the print preview).
    Result runToEnd();
    // Stops: the bands in the engine are withdrawn, the printer's job is aborted and `finished`
    // reports `Cancelled`. Does nothing once the job is over.
    void cancel();

    bool isRunning() const { return m_running; }
    const PrintOptions& options() const { return m_options; }
    // Bands asked of the engine so far, counting each ask once (a band asked again after the
    // engine restarted counts again).
    int bandsRequested() const { return m_bandsRequested; }
    int sheetsDone() const { return m_sheet; }
    // Why the job failed or was cancelled; empty otherwise.
    QString message() const { return m_message; }

    // The size of the bands for a page of `width` x `height` render pixels when a slot holds
    // `slotBytes`: as wide as the page, up to the protocol's side limit, and as high as fits.
    static QSize bandSize(quint32 width, quint32 height, quint32 slotBytes);

signals:
    // `value` of `maximum`: sheets done, with the bands of the current one as a fraction.
    void progress(int value, int maximum);
    void finished(PrintJob::Result result, const QString& message);

private slots:
    void onTileReady(quint64 request);
    void onRequestFailed(quint64 request, const QString& message);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void onEngineRestarted();
    void onFailed(const QString& reason);

private:
    bool beginSheet(QString& error);
    void pump();
    void drawBand(const QRect& band, const QImage& image);
    void sheetFinished();
    void emitProgress();
    // Ends the job with `result`: withdraws the bands in flight, aborts or closes the printer.
    void finish(Result result, const QString& message);

    EngineSession& m_session;
    QPrinter& m_printer;
    QList<quint32> m_pages;
    PrintOptions m_options;
    QPainter m_painter;
    bool m_running = false;
    bool m_waitingForEngine = false;
    int m_sheet = 0;
    int m_bandsRequested = 0;
    QString m_message;

    // The sheet being printed.
    QTransform m_transform;
    bool m_scaled = false;
    QRectF m_clip;
    QList<QRect> m_queue;
    QHash<quint64, QRect> m_inflight;
    quint32 m_bandsInSheet = 0;
    quint32 m_bandsDrawn = 0;
    quint32 m_renderWidth = 0;
    quint32 m_renderHeight = 0;
    float m_renderScale = 1.0F;
    QImage m_image;
};

} // namespace vellora
