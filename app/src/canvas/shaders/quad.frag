#version 440

layout(location = 0) in vec2 v_uv;

layout(location = 0) out vec4 fragColor;

layout(binding = 1) uniform sampler2D tex;

void main() {
    // Tiles are BGRx: the x byte is not alpha, so force opaque.
    fragColor = vec4(texture(tex, v_uv).rgb, 1.0);
}
