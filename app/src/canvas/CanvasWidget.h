// Draws the canvas with QRhi: grey background, a white quad per page, and a textured quad per tile
// the engine has finished. All decisions (what is visible, what to ask for) are the controller's;
// this class only turns a Frame into draw calls and keeps a GPU copy of tiles it has shown.
//
// Rendering backend: Qt's default for the platform, or the one named in the environment variable
// `VELLORA_RHI` (`null`, `opengl`, `vulkan`, `d3d11`, `d3d12`, `metal`). QRhiWidget needs a
// platform plugin that can run QRhi: the offscreen platform cannot, so there the widget stays
// blank.
#pragma once

#include "canvas/CanvasController.h"

#include <QElapsedTimer>
#include <QHash>
#include <QRhiWidget>
#include <memory>

class QRhiBuffer;
class QRhiGraphicsPipeline;
class QRhiSampler;
class QRhiShaderResourceBindings;
class QRhiTexture;

namespace vellora {

class CanvasWidget : public QRhiWidget {
    Q_OBJECT

public:
    // Textures kept on the GPU (1 MiB each); the least recently drawn go first.
    static constexpr int kMaxTextures = 192;
    // New tiles finished (uploaded) per frame, so a burst of finished tiles cannot stall one frame.
    static constexpr int kUploadsPerFrame = 6;
    // A tile is uploaded in two frames (read, then upload), and a frame does a half only if it is
    // expected to fit in this long, from what the last ones cost: a count does not bound time, and
    // `render()` has 8 ms in all (UiWatchdog::kBudgetMs). Every frame does at least one half.
    static constexpr double kUploadBudgetMs = 3.0;
    // Evicted textures kept for reuse.
    static constexpr int kMaxSpareTextures = 8;
    // Quads the vertex buffer holds (pages and tiles of one frame).
    static constexpr int kMaxQuads = 2048;

    CanvasWidget(EngineSession* session, CanvasController* controller, QWidget* parent = nullptr);
    ~CanvasWidget() override;

    // Tiles shown and textures held, for tests.
    int textureCount() const { return static_cast<int>(m_textures.size()); }
    quint64 framesRendered() const { return m_frames; }
    // Time spent in `render()` (preparing tiles and recording the frame, not presenting it): the
    // last frame, the slowest, and the part of the last one spent copying and uploading tiles.
    double lastRenderMs() const { return m_lastRenderMs; }
    double slowestRenderMs() const { return m_slowestRenderMs; }
    double lastUploadMs() const { return m_lastUploadMs; }

signals:
    // Emitted at the end of every `render()` with the time it took, in milliseconds (the UI
    // watchdog's frame measurement).
    void frameRendered(double ms);

protected:
    void initialize(QRhiCommandBuffer* cb) override;
    void render(QRhiCommandBuffer* cb) override;
    void releaseResources() override;
    bool event(QEvent* event) override;

private:
    struct GpuTile {
        QRhiTexture* texture = nullptr;
        QRhiShaderResourceBindings* bindings = nullptr;
        quint64 lastUsed = 0;
    };

    QRhiShaderResourceBindings* bindingsFor(QRhiTexture* texture);
    void evictTextures();
    void releaseTextures();

    EngineSession* m_session;
    CanvasController* m_controller;

    QRhiBuffer* m_vertices = nullptr;
    QRhiBuffer* m_uniforms = nullptr;
    QRhiSampler* m_sampler = nullptr;
    QRhiGraphicsPipeline* m_pipeline = nullptr;
    QRhiTexture* m_white = nullptr;
    QRhiShaderResourceBindings* m_whiteBindings = nullptr;
    bool m_whiteUploaded = false;

    QHash<TileId, GpuTile> m_textures;
    // Evicted textures kept to be written again: creating a texture costs a driver call that is
    // sometimes slow, and a frame has 8 ms.
    QVector<GpuTile> m_spare;
    QByteArray m_scratch;
    quint64 m_frames = 0;
    double m_lastRenderMs = 0.0;
    double m_slowestRenderMs = 0.0;
    double m_lastUploadMs = 0.0;
    // What the two halves of a tile upload have cost lately (moving averages, ms): reading the
    // pixels from the shared region, and creating the texture and uploading them. A frame does a
    // half only if it is expected to fit the budget, because a budget checked after the fact is
    // overshot by a whole half.
    double m_readEstimateMs = 1.5;
    double m_gpuEstimateMs = 2.0;
    // The tile read in the last frame, waiting to be uploaded in this one (`m_scratch` has its
    // pixels).
    bool m_hasStaged = false;
    TileId m_stagedId;
};

} // namespace vellora
