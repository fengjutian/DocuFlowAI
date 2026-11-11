struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) viewport: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) pixel_position: vec2<f32>,
    @location(2) @interpolate(flat) clip: vec4<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0)
    );
    let pixel = input.rect.xy + corners[input.vertex_index] * input.rect.zw;
    var output: VertexOutput;
    output.position = vec4(
        pixel.x / input.viewport.x * 2.0 - 1.0,
        1.0 - pixel.y / input.viewport.y * 2.0,
        0.0, 1.0
    );
    output.color = input.color;
    output.pixel_position = pixel;
    output.clip = input.clip;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.pixel_position.x < input.clip.x || input.pixel_position.y < input.clip.y ||
        input.pixel_position.x >= input.clip.z || input.pixel_position.y >= input.clip.w) {
        discard;
    }
    return input.color;
}
