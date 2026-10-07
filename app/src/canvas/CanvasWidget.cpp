#include "canvas/CanvasWidget.h"

#include <QEvent>
#include <QFile>
#include <QMatrix4x4>
#include <QtCore/qglobal.h>
#include <algorithm>
#include <rhi/qrhi.h>

namespace vellora {

namespace {

constexpr int kTilePixels = 512;
constexpr int kFloatsPerVertex = 4; // x, y, u, v
constexpr int kVerticesPerQuad = 6;
constexpr quint32 kBytesPerQuad = kFloatsPerVertex * kVerticesPerQuad * sizeof(float);

QShader loadShader(const QString& name) {
    QFile file(name);
    if (!file.open(QIODevice::ReadOnly)) {
        qCritical("canvas: missing shader %s", qPrintable(name));
        return {};
    }
    return QShader::fromSerialized(file.readAll());
}

// Two triangles for `rect` showing `uv` of a texture.
void appendQuad(QVector<float>& vertices, const QRectF& rect, const QRectF& uv) {
    const auto x0 = static_cast<float>(rect.left());
    const auto y0 = static_cast<float>(rect.top());
    const auto x1 = static_cast<float>(rect.right());
    const auto y1 = static_cast<float>(rect.bottom());
    const auto u0 = static_cast<float>(uv.left());
    const auto v0 = static_cast<float>(uv.top());
    const auto u1 = static_cast<float>(uv.right());
    const auto v1 = static_cast<float>(uv.bottom());
    vertices << x0 << y0 << u0 << v0 << x1 << y0 << u1 << v0 << x0 << y1 << u0 << v1;
    vertices << x1 << y0 << u1 << v0 << x1 << y1 << u1 << v1 << x0 << y1 << u0 << v1;
}

std::optional<QRhiWidget::Api> apiFromEnvironment() {
    const QByteArray name = qgetenv("VELLORA_RHI").toLower();
    if (name == "null") {
        return QRhiWidget::Api::Null;
    }
    if (name == "opengl") {
        return QRhiWidget::Api::OpenGL;
    }
    if (name == "vulkan") {
        return QRhiWidget::Api::Vulkan;
    }
    if (name == "d3d11") {
        return QRhiWidget::Api::Direct3D11;
    }
    if (name == "d3d12") {
        return QRhiWidget::Api::Direct3D12;
    }
    if (name == "metal") {
        return QRhiWidget::Api::Metal;
    }
    return std::nullopt;
}

} // namespace

CanvasWidget::CanvasWidget(EngineSession* session, CanvasController* controller, QWidget* parent)
    : QRhiWidget(parent), m_session(session), m_controller(controller) {
    if (const auto api = apiFromEnvironment()) {
        setApi(*api);
    }
    connect(controller, &CanvasController::viewChanged, this, qOverload<>(&QWidget::update));
}

CanvasWidget::~CanvasWidget() {
    releaseResources();
}

bool CanvasWidget::event(QEvent* event) {
    if (event->type() == QEvent::DevicePixelRatioChange) {
        m_controller->setDevicePixelRatio(devicePixelRatioF());
    }
    return QRhiWidget::event(event);
}

void CanvasWidget::releaseTextures() {
    for (auto it = m_textures.begin(); it != m_textures.end(); ++it) {
        it->bindings->deleteLater();
        it->texture->deleteLater();
    }
    m_textures.clear();
}

void CanvasWidget::releaseResources() {
    releaseTextures();
    // `releaseAndDestroyLater` lets a frame that is still in flight finish with the resource.
    for (QRhiResource* resource : std::initializer_list<QRhiResource*>{
             m_whiteBindings, m_white, m_pipeline, m_sampler, m_uniforms, m_vertices}) {
        if (resource) {
            resource->deleteLater();
        }
    }
    m_whiteBindings = nullptr;
    m_white = nullptr;
    m_pipeline = nullptr;
    m_sampler = nullptr;
    m_uniforms = nullptr;
    m_vertices = nullptr;
    m_whiteUploaded = false;
}

void CanvasWidget::initialize(QRhiCommandBuffer* cb) {
    Q_UNUSED(cb);
    // Also called when the render target changed: everything is rebuilt, tiles come back as they
    // are drawn (the pixels are still in the client cache).
    releaseResources();
    QRhi* rhi = this->rhi();

    m_vertices =
        rhi->newBuffer(QRhiBuffer::Dynamic, QRhiBuffer::VertexBuffer, kBytesPerQuad * kMaxQuads);
    m_vertices->create();
    m_uniforms = rhi->newBuffer(QRhiBuffer::Dynamic, QRhiBuffer::UniformBuffer, 64);
    m_uniforms->create();
    m_sampler = rhi->newSampler(QRhiSampler::Linear, QRhiSampler::Linear, QRhiSampler::None,
                                QRhiSampler::ClampToEdge, QRhiSampler::ClampToEdge);
    m_sampler->create();

    m_white = rhi->newTexture(QRhiTexture::RGBA8, QSize(1, 1));
    m_white->create();
    m_whiteBindings = bindingsFor(m_white);

    m_pipeline = rhi->newGraphicsPipeline();
    m_pipeline->setShaderStages(
        {{QRhiShaderStage::Vertex, loadShader(":/shaders/quad.vert.qsb")},
         {QRhiShaderStage::Fragment, loadShader(":/shaders/quad.frag.qsb")}});
    QRhiVertexInputLayout layout;
    layout.setBindings({{kFloatsPerVertex * sizeof(float)}});
    layout.setAttributes({{0, 0, QRhiVertexInputAttribute::Float2, 0},
                          {0, 1, QRhiVertexInputAttribute::Float2, 2 * sizeof(float)}});
    m_pipeline->setVertexInputLayout(layout);
    m_pipeline->setShaderResourceBindings(m_whiteBindings);
    m_pipeline->setRenderPassDescriptor(renderTarget()->renderPassDescriptor());
    m_pipeline->create();
}

QRhiShaderResourceBindings* CanvasWidget::bindingsFor(QRhiTexture* texture) {
    QRhiShaderResourceBindings* bindings = rhi()->newShaderResourceBindings();
    bindings->setBindings({
        QRhiShaderResourceBinding::uniformBuffer(0, QRhiShaderResourceBinding::VertexStage,
                                                 m_uniforms),
        QRhiShaderResourceBinding::sampledTexture(1, QRhiShaderResourceBinding::FragmentStage,
                                                  texture, m_sampler),
    });
    bindings->create();
    return bindings;
}

void CanvasWidget::evictTextures() {
    while (m_textures.size() > kMaxTextures) {
        auto oldest = m_textures.begin();
        for (auto it = m_textures.begin(); it != m_textures.end(); ++it) {
            if (it->lastUsed < oldest->lastUsed) {
                oldest = it;
            }
        }
        // Never one that this frame just drew.
        if (oldest->lastUsed == m_frames) {
            return;
        }
        oldest->bindings->deleteLater();
        oldest->texture->deleteLater();
        m_textures.erase(oldest);
    }
}

void CanvasWidget::render(QRhiCommandBuffer* cb) {
    ++m_frames;
    QRhi* rhi = this->rhi();
    QRhiResourceUpdateBatch* updates = rhi->nextResourceUpdateBatch();

    if (!m_whiteUploaded) {
        const QByteArray white(4, '\xFF');
        updates->uploadTexture(
            m_white, QRhiTextureUploadEntry(0, 0, QRhiTextureSubresourceUploadDescription(white)));
        m_whiteUploaded = true;
    }

    const QSize logical = size();
    QMatrix4x4 mvp = rhi->clipSpaceCorrMatrix();
    mvp.ortho(0.0F, static_cast<float>(logical.width()), static_cast<float>(logical.height()), 0.0F,
              -1.0F, 1.0F);
    updates->updateDynamicBuffer(m_uniforms, 0, 64, mvp.constData());

    const Frame frame = m_controller->frame();

    // Pages first (white placeholders), then the tiles that are ready on top of them.
    struct Draw {
        QRhiShaderResourceBindings* bindings;
        int firstQuad;
    };
    QVector<Draw> draws;
    QVector<float> vertices;
    const QRectF whole(0.0, 0.0, 1.0, 1.0);
    for (const PageDraw& page : frame.pages) {
        if (draws.size() >= kMaxQuads) {
            break;
        }
        draws.append({m_whiteBindings, static_cast<int>(vertices.size() / (kFloatsPerVertex * 6))});
        appendQuad(vertices, page.rect, whole);
    }

    int uploads = 0;
    bool waiting = false;
    for (const TileDraw& tile : frame.tiles) {
        if (draws.size() >= kMaxQuads) {
            break;
        }
        auto it = m_textures.find(tile.id());
        if (it == m_textures.end()) {
            if (uploads >= kUploadsPerFrame) {
                waiting = true;
                continue;
            }
            if (!m_session->readTile(tile.page, tile.scale, tile.x, tile.y, m_scratch)) {
                continue; // not rendered yet; the page stays white until tileReady
            }
            GpuTile gpu;
            gpu.texture = rhi->newTexture(QRhiTexture::BGRA8, QSize(kTilePixels, kTilePixels));
            gpu.texture->create();
            gpu.bindings = bindingsFor(gpu.texture);
            // The slot is a whole megabyte; the tile is its first 512 x 512 x 4 bytes.
            updates->uploadTexture(
                gpu.texture,
                QRhiTextureUploadEntry(0, 0,
                                       QRhiTextureSubresourceUploadDescription(
                                           m_scratch.constData(), kTilePixels * kTilePixels * 4)));
            it = m_textures.insert(tile.id(), gpu);
            ++uploads;
        }
        it->lastUsed = m_frames;
        draws.append({it->bindings, static_cast<int>(vertices.size() / (kFloatsPerVertex * 6))});
        appendQuad(vertices, tile.dest, tile.uv);
    }
    evictTextures();

    if (!vertices.isEmpty()) {
        updates->updateDynamicBuffer(m_vertices, 0,
                                     static_cast<quint32>(vertices.size() * sizeof(float)),
                                     vertices.constData());
    }

    const QColor background(0xC8, 0xC8, 0xC8);
    cb->beginPass(renderTarget(), background, {1.0F, 0}, updates);
    cb->setGraphicsPipeline(m_pipeline);
    const QSize pixels = renderTarget()->pixelSize();
    cb->setViewport(
        {0.0F, 0.0F, static_cast<float>(pixels.width()), static_cast<float>(pixels.height())});
    for (const Draw& draw : draws) {
        cb->setShaderResources(draw.bindings);
        const QRhiCommandBuffer::VertexInput input(m_vertices, draw.firstQuad * kBytesPerQuad);
        cb->setVertexInput(0, 1, &input);
        cb->draw(kVerticesPerQuad);
    }
    cb->endPass();

    if (waiting) {
        update(); // more tiles to upload: continue next frame
    }
}

} // namespace vellora
