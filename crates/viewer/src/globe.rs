//! OpenGL renderer for the geodesic globe.
//!
//! Each cell is drawn as a fan of triangles from its centre to its Voronoi
//! corners. A per-vertex "edge" attribute is 0 at the centre and 1 on the
//! polygon boundary, and the fragment shader turns values near 1 into
//! pixel-width cell borders, so no separate line pass is needed.

use egui_glow::glow::{self, HasContext};

use dirtbox_grid::{CellId, GeodesicGrid};

use crate::camera::{Mat4, V3};
use crate::overlay::Rgb;

const VERTEX_SHADER: &str = r#"
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_colour;
layout(location = 2) in vec2 a_edge_cell;
uniform mat4 u_view_proj;
uniform float u_hovered;
out vec3 v_pos;
out vec3 v_colour;
out float v_edge;
flat out float v_hovered;
void main() {
    v_pos = a_pos;
    v_colour = a_colour;
    v_edge = a_edge_cell.x;
    v_hovered = abs(a_edge_cell.y - u_hovered) < 0.5 ? 1.0 : 0.0;
    gl_Position = u_view_proj * vec4(a_pos, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"
in vec3 v_pos;
in vec3 v_colour;
in float v_edge;
flat in float v_hovered;
uniform vec3 u_eye;
uniform vec3 u_sun;
uniform float u_sunlit;
uniform float u_borders;
out vec4 frag_colour;
void main() {
    vec3 n = normalize(v_pos);
    vec3 view = normalize(u_eye - v_pos);

    // Sunlit: day side lit by the sun with a soft terminator; the night side
    // keeps enough light for the map to stay readable. Even: a headlight.
    float sun = dot(n, u_sun);
    float day = 0.38 + 0.62 * smoothstep(-0.08, 0.35, sun);
    float even = 0.6 + 0.4 * max(dot(n, view), 0.0);
    vec3 colour = v_colour * mix(even, day, u_sunlit);

    // Cell borders, about one pixel wide at any zoom.
    float w = fwidth(v_edge);
    float border = smoothstep(1.0 - 1.6 * w, 1.0 - 0.4 * w, v_edge);
    colour = mix(colour, colour * 0.45, border * u_borders);

    if (v_hovered > 0.5) {
        colour = mix(colour, vec3(1.0), 0.3 + 0.5 * border);
    }

    // Thin atmosphere rim.
    float rim = pow(1.0 - max(dot(n, view), 0.0), 4.0);
    colour += vec3(0.30, 0.50, 0.95) * rim * 0.55;

    frag_colour = vec4(colour, 1.0);
}
"#;

/// CPU-side mesh for one grid.
pub struct GlobeMesh {
    positions: Vec<f32>,
    edge_cell: Vec<f32>,
    indices: Vec<u32>,
    /// Owning cell of each vertex, for expanding per-cell colours.
    vertex_cell: Vec<CellId>,
}

impl GlobeMesh {
    pub fn build(grid: &GeodesicGrid) -> Self {
        let n = grid.cell_count();
        let mut mesh = GlobeMesh {
            positions: Vec::with_capacity(n * 7 * 3),
            edge_cell: Vec::with_capacity(n * 7 * 2),
            indices: Vec::with_capacity(n * 18),
            vertex_cell: Vec::with_capacity(n * 7),
        };
        for cell in grid.cells() {
            let base = mesh.vertex_cell.len() as u32;
            let centre = grid.position(cell);
            mesh.push(centre.x, centre.y, centre.z, 0.0, cell);
            let degree = grid.corners(cell).len() as u32;
            for corner in grid.corners(cell) {
                mesh.push(corner.x, corner.y, corner.z, 1.0, cell);
            }
            for k in 0..degree {
                mesh.indices
                    .extend([base, base + 1 + k, base + 1 + (k + 1) % degree]);
            }
        }
        mesh
    }

    fn push(&mut self, x: f64, y: f64, z: f64, edge: f32, cell: CellId) {
        self.positions.extend([x as f32, y as f32, z as f32]);
        // Cell ids up to 2²⁴ are exact in f32; the grid caps far below that.
        self.edge_cell.extend([edge, cell as f32]);
        self.vertex_cell.push(cell);
    }

    pub fn expand_colours(&self, cell_colours: &[Rgb]) -> Vec<u8> {
        self.vertex_cell
            .iter()
            .flat_map(|&c| cell_colours[c as usize])
            .collect()
    }
}

/// Per-frame values the renderer needs.
#[derive(Clone, Copy)]
pub struct FrameUniforms {
    pub view_proj: Mat4,
    pub eye: V3,
    pub sun: V3,
    pub sunlit: bool,
    pub borders: f32,
    pub hovered: Option<CellId>,
}

pub struct GlobeRenderer {
    program: glow::Program,
    vao: glow::VertexArray,
    position_buffer: glow::Buffer,
    edge_cell_buffer: glow::Buffer,
    colour_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
    index_count: i32,
    pending_mesh: Option<(Vec<f32>, Vec<f32>, Vec<u32>)>,
    pending_colours: Option<Vec<u8>>,
}

impl GlobeRenderer {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // GLSL 3.30 core works on desktop GL 3.3+ (Windows, Linux, macOS).
        let header = "#version 330 core\n";
        unsafe {
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX_SHADER),
                (glow::FRAGMENT_SHADER, FRAGMENT_SHADER),
            ] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(format!("globe shader: {}", gl.get_shader_info_log(shader)));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!(
                    "globe program: {}",
                    gl.get_program_info_log(program)
                ));
            }
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }

            let vao = gl.create_vertex_array()?;
            let position_buffer = gl.create_buffer()?;
            let colour_buffer = gl.create_buffer()?;
            let edge_cell_buffer = gl.create_buffer()?;
            let index_buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(position_buffer));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(colour_buffer));
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::UNSIGNED_BYTE, true, 0, 0);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(edge_cell_buffer));
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, 0, 0);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            Ok(Self {
                program,
                vao,
                position_buffer,
                edge_cell_buffer,
                colour_buffer,
                index_buffer,
                index_count: 0,
                pending_mesh: None,
                pending_colours: None,
            })
        }
    }

    /// Queues a new mesh; it is uploaded on the next paint.
    pub fn set_mesh(&mut self, mesh: &GlobeMesh) {
        self.pending_mesh = Some((
            mesh.positions.clone(),
            mesh.edge_cell.clone(),
            mesh.indices.clone(),
        ));
    }

    /// Queues new vertex colours (3 bytes per vertex).
    pub fn set_colours(&mut self, colours: Vec<u8>) {
        self.pending_colours = Some(colours);
    }

    pub fn paint(&mut self, gl: &glow::Context, u: &FrameUniforms, viewport_px: [i32; 4]) {
        unsafe {
            if let Some((positions, edge_cell, indices)) = self.pending_mesh.take() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.position_buffer));
                gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    as_bytes(&positions),
                    glow::STATIC_DRAW,
                );
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.edge_cell_buffer));
                gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    as_bytes(&edge_cell),
                    glow::STATIC_DRAW,
                );
                gl.bind_vertex_array(Some(self.vao));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    as_bytes(&indices),
                    glow::STATIC_DRAW,
                );
                gl.bind_vertex_array(None);
                self.index_count = indices.len() as i32;
            }
            if let Some(colours) = self.pending_colours.take() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.colour_buffer));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &colours, glow::DYNAMIC_DRAW);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            if self.index_count == 0 {
                return;
            }

            let [x, y, w, h] = viewport_px;
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x, y, w, h);
            // glClear honours the depth write mask, so enable it first.
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.enable(glow::CULL_FACE);
            gl.cull_face(glow::BACK);
            gl.front_face(glow::CCW);
            gl.disable(glow::BLEND);

            gl.use_program(Some(self.program));
            let loc = |name| gl.get_uniform_location(self.program, name);
            gl.uniform_matrix_4_f32_slice(loc("u_view_proj").as_ref(), false, &u.view_proj.0);
            gl.uniform_3_f32(loc("u_eye").as_ref(), u.eye.0, u.eye.1, u.eye.2);
            gl.uniform_3_f32(loc("u_sun").as_ref(), u.sun.0, u.sun.1, u.sun.2);
            gl.uniform_1_f32(loc("u_sunlit").as_ref(), if u.sunlit { 1.0 } else { 0.0 });
            gl.uniform_1_f32(loc("u_borders").as_ref(), u.borders);
            gl.uniform_1_f32(
                loc("u_hovered").as_ref(),
                u.hovered.map_or(-1.0, |c| c as f32),
            );

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
            gl.depth_mask(false);
        }
    }

    pub fn destroy(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            for b in [
                self.position_buffer,
                self.edge_cell_buffer,
                self.colour_buffer,
                self.index_buffer,
            ] {
                gl.delete_buffer(b);
            }
        }
    }
}

fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    // SAFETY: only used with f32/u32, which have no padding or invalid bit
    // patterns; the slice covers exactly the vector's initialised elements.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast(), std::mem::size_of_val(v)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_has_a_fan_per_cell() {
        let grid = GeodesicGrid::new(4);
        let mesh = GlobeMesh::build(&grid);
        let corners: usize = grid.cells().map(|c| grid.corners(c).len()).sum();
        assert_eq!(mesh.vertex_cell.len(), grid.cell_count() + corners);
        assert_eq!(mesh.indices.len(), 3 * corners);
        let colours = mesh.expand_colours(&vec![[1, 2, 3]; grid.cell_count()]);
        assert_eq!(colours.len(), 3 * mesh.vertex_cell.len());
    }
}
