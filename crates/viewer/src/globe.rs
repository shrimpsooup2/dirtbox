//! OpenGL renderer for the geodesic globe.
//!
//! Each cell is a fan of triangles from its centre to its Voronoi corners. A
//! per-vertex "edge" attribute is 0 at the centre and 1 on the polygon
//! boundary; the fragment shader turns values near 1 into pixel-width cell
//! borders. Vertices are lifted by (exaggerated) terrain height: centres by
//! their cell's height, corners by the mean of the three cells that meet
//! there, which gives a continuous surface.

use egui_glow::glow::{self, HasContext};

use dirtbox_core::Vec3;
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
uniform float u_relief;
uniform vec4 u_brush;
out vec4 frag_colour;
void main() {
    vec3 up = normalize(v_pos);
    vec3 view = normalize(u_eye - v_pos);
    // Facet normal from screen-space derivatives shows the relief.
    vec3 facet = normalize(cross(dFdx(v_pos), dFdy(v_pos)));
    if (dot(facet, up) < 0.0) facet = -facet;
    vec3 n = normalize(mix(up, facet, u_relief));

    // Day side lit by the star with a soft terminator (from the sphere
    // normal, so mountains don't light up at night); relief adds shading.
    float day = smoothstep(-0.08, 0.35, dot(up, u_sun));
    float shade = 0.75 + 0.25 * clamp(dot(n, u_sun) / max(dot(up, u_sun), 0.2), 0.0, 1.6);
    float sunlit = (0.38 + 0.62 * day) * mix(1.0, shade, u_relief * day);
    float even = 0.6 + 0.4 * max(dot(n, view), 0.0);
    vec3 colour = v_colour * mix(even, sunlit, u_sunlit);

    float w = fwidth(v_edge);
    float border = smoothstep(1.0 - 1.6 * w, 1.0 - 0.4 * w, v_edge);
    colour = mix(colour, colour * 0.45, border * u_borders);

    if (v_hovered > 0.5) {
        colour = mix(colour, vec3(1.0), 0.3 + 0.5 * border);
    }

    // Terraforming brush preview.
    if (u_brush.w > 0.0) {
        float d = acos(clamp(dot(up, u_brush.xyz), -1.0, 1.0));
        float wd = fwidth(d);
        float ring = 1.0 - smoothstep(0.0, 2.0 * wd, abs(d - u_brush.w));
        float inside = 1.0 - step(u_brush.w, d);
        colour = mix(colour, vec3(1.0, 0.86, 0.45), inside * 0.12 + ring * 0.85);
    }

    float rim = pow(1.0 - max(dot(up, view), 0.0), 4.0);
    colour += vec3(0.30, 0.50, 0.95) * rim * 0.55;
    frag_colour = vec4(colour, 1.0);
}
"#;

/// CPU-side mesh for one grid.
pub struct GlobeMesh {
    /// Unit-sphere position of each vertex.
    unit: Vec<Vec3>,
    /// For corner vertices, the mesh triangle whose three cells set the
    /// height; `u32::MAX` for cell centres.
    corner_triangle: Vec<u32>,
    edge_cell: Vec<f32>,
    indices: Vec<u32>,
    /// Owning cell of each vertex, for expanding per-cell colours.
    vertex_cell: Vec<CellId>,
}

impl GlobeMesh {
    pub fn build(grid: &GeodesicGrid) -> Self {
        let n = grid.cell_count();
        let mut mesh = GlobeMesh {
            unit: Vec::with_capacity(n * 7),
            corner_triangle: Vec::with_capacity(n * 7),
            edge_cell: Vec::with_capacity(n * 7 * 2),
            indices: Vec::with_capacity(n * 18),
            vertex_cell: Vec::with_capacity(n * 7),
        };
        for cell in grid.cells() {
            let base = mesh.vertex_cell.len() as u32;
            mesh.push(grid.position(cell), u32::MAX, 0.0, cell);
            let degree = grid.corners(cell).len() as u32;
            for (corner, &tri) in grid.corners(cell).zip(grid.corner_triangles(cell)) {
                mesh.push(corner, tri, 1.0, cell);
            }
            for k in 0..degree {
                mesh.indices
                    .extend([base, base + 1 + k, base + 1 + (k + 1) % degree]);
            }
        }
        mesh
    }

    fn push(&mut self, p: Vec3, tri: u32, edge: f32, cell: CellId) {
        self.unit.push(p);
        self.corner_triangle.push(tri);
        // Cell ids up to 2²⁴ are exact in f32; the grid caps far below that.
        self.edge_cell.extend([edge, cell as f32]);
        self.vertex_cell.push(cell);
    }

    /// Vertex positions with each cell lifted by `height` (m, per cell) ×
    /// `exaggeration`, on a unit globe for a planet of `radius` m.
    pub fn positions(
        &self,
        grid: &GeodesicGrid,
        height: &[f64],
        radius: f64,
        exaggeration: f64,
    ) -> Vec<f32> {
        let scale = exaggeration / radius;
        let tris = grid.triangles();
        let mut out = Vec::with_capacity(self.unit.len() * 3);
        for (v, &p) in self.unit.iter().enumerate() {
            let h = match self.corner_triangle[v] {
                u32::MAX => height[self.vertex_cell[v] as usize],
                t => {
                    tris[t as usize]
                        .iter()
                        .map(|&c| height[c as usize])
                        .sum::<f64>()
                        / 3.0
                }
            };
            let q = p * (1.0 + scale * h);
            out.extend([q.x as f32, q.y as f32, q.z as f32]);
        }
        out
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
    pub relief: f32,
    pub hovered: Option<CellId>,
    /// Brush centre (unit vector) and radius (radians), if previewing.
    pub brush: Option<(V3, f32)>,
}

pub struct GlobeRenderer {
    program: glow::Program,
    vao: glow::VertexArray,
    position_buffer: glow::Buffer,
    edge_cell_buffer: glow::Buffer,
    colour_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
    index_count: i32,
    pending_topology: Option<(Vec<f32>, Vec<u32>)>,
    pending_positions: Option<Vec<f32>>,
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
                pending_topology: None,
                pending_positions: None,
                pending_colours: None,
            })
        }
    }

    /// Queues a new mesh topology; uploaded on the next paint.
    pub fn set_mesh(&mut self, mesh: &GlobeMesh) {
        self.pending_topology = Some((mesh.edge_cell.clone(), mesh.indices.clone()));
    }

    /// Queues new vertex positions (3 floats per vertex).
    pub fn set_positions(&mut self, positions: Vec<f32>) {
        self.pending_positions = Some(positions);
    }

    /// Queues new vertex colours (3 bytes per vertex).
    pub fn set_colours(&mut self, colours: Vec<u8>) {
        self.pending_colours = Some(colours);
    }

    pub fn paint(&mut self, gl: &glow::Context, u: &FrameUniforms, viewport_px: [i32; 4]) {
        unsafe {
            if let Some((edge_cell, indices)) = self.pending_topology.take() {
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
            if let Some(positions) = self.pending_positions.take() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.position_buffer));
                gl.buffer_data_u8_slice(
                    glow::ARRAY_BUFFER,
                    as_bytes(&positions),
                    glow::DYNAMIC_DRAW,
                );
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
            gl.uniform_1_f32(loc("u_relief").as_ref(), u.relief);
            gl.uniform_1_f32(
                loc("u_hovered").as_ref(),
                u.hovered.map_or(-1.0, |c| c as f32),
            );
            let (bc, br) = u.brush.unwrap_or(((0.0, 0.0, 1.0), -1.0));
            gl.uniform_4_f32(loc("u_brush").as_ref(), bc.0, bc.1, bc.2, br);

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
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

    #[test]
    fn relief_is_continuous_at_shared_corners() {
        let grid = GeodesicGrid::new(5);
        let mesh = GlobeMesh::build(&grid);
        let height: Vec<f64> = grid.cells().map(|c| (c % 7) as f64 * 1000.0).collect();
        let pos = mesh.positions(&grid, &height, 6.371e6, 50.0);
        // Vertices for the same corner (same triangle) coincide exactly.
        let mut seen = std::collections::HashMap::new();
        for (v, &t) in mesh.corner_triangle.iter().enumerate() {
            if t == u32::MAX {
                continue;
            }
            let p = [pos[3 * v], pos[3 * v + 1], pos[3 * v + 2]];
            if let Some(q) = seen.insert(t, p) {
                assert_eq!(p, q, "corner of triangle {t} has two heights");
            }
        }
    }
}
