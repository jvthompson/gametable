//! A recorder with the drawing calls the original made through `love.graphics`: colour, line
//! width, transforms, shapes, textured quads, scissor, stencil and custom fragment programs. It
//! produces vertices in physical pixels plus batched draw commands for the GPU renderer.

use super::tessellation::{self, ArcMode, DrawMode, LineStyle, Point, ShapeVertex};

/// A colour with components in 0..1, as `love.graphics.setColor` takes them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

impl Color {
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);

    pub const fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Color {
        Color { red, green, blue, alpha }
    }

    /// From an ARGB integer (the format colours are stored in).
    pub fn from_argb(argb: u32) -> Color {
        let channel = |shift: u32| ((argb >> shift) & 0xFF) as f32 / 255.0;
        Color::rgba(channel(16), channel(8), channel(0), channel(24))
    }

    pub fn with_alpha_multiplied(self, factor: f32) -> Color {
        Color { alpha: self.alpha * factor, ..self }
    }
}

/// A 2D affine transform: `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Transform {
    pub const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn apply(&self, point: Point) -> Point {
        Point::new(self.a * point.x + self.c * point.y + self.tx, self.b * point.x + self.d * point.y + self.ty)
    }

    /// `self` followed by `other` applied first (LÖVE's `transform * local`).
    pub fn then(&self, other: &Transform) -> Transform {
        Transform {
            a: self.a * other.a + self.c * other.b,
            b: self.b * other.a + self.d * other.b,
            c: self.a * other.c + self.c * other.d,
            d: self.b * other.c + self.d * other.d,
            tx: self.a * other.tx + self.c * other.ty + self.tx,
            ty: self.b * other.tx + self.d * other.ty + self.ty,
        }
    }

    pub fn translation(x: f32, y: f32) -> Transform {
        Transform { tx: x, ty: y, ..Transform::IDENTITY }
    }

    pub fn scaling(x: f32, y: f32) -> Transform {
        Transform { a: x, d: y, ..Transform::IDENTITY }
    }

    pub fn rotation(angle: f32) -> Transform {
        let (sine, cosine) = angle.sin_cos();
        Transform { a: cosine, b: sine, c: -sine, d: cosine, tx: 0.0, ty: 0.0 }
    }

    /// The inverse, for turning screen points back into local ones.
    pub fn inverse(&self) -> Transform {
        let determinant = self.a * self.d - self.b * self.c;
        if determinant == 0.0 {
            return Transform::IDENTITY;
        }
        let inverse_determinant = 1.0 / determinant;
        let a = self.d * inverse_determinant;
        let b = -self.b * inverse_determinant;
        let c = -self.c * inverse_determinant;
        let d = self.a * inverse_determinant;
        Transform { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) }
    }
}

/// A texture the renderer knows about. `0` is the built-in white pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextureId(pub u32);

impl TextureId {
    pub const WHITE: TextureId = TextureId(0);

    /// A new id, unique for the whole process. Ids are handed out before a texture exists on the
    /// GPU so draw lists can refer to textures (such as glyph atlas pages) created at flush time.
    pub fn allocate() -> TextureId {
        static NEXT_TEXTURE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        TextureId(NEXT_TEXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// A texture's id and size, enough to draw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureInfo {
    pub id: TextureId,
    pub width: f32,
    pub height: f32,
}

/// Part of a texture in pixels (LÖVE's `Quad`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The fragment program a draw uses (the original's shaders).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FragmentProgram {
    /// Texture × vertex colour.
    #[default]
    Plain,
    /// Luminance greyscale, keeping alpha (flipped image tokens).
    Greyscale,
    /// Discards outside rounded corners of a `size` destination (card images).
    RoundedImage { quad_rect: [f32; 4], size: [f32; 2], radius: f32 },
    /// The procedural cloth grain on the table felt.
    Felt { zoom: f32, world_offset: [f32; 2] },
}

/// LÖVE's stencil compare modes. A draw passes where the pixel's stencil value compares to the
/// given value as named: `Greater` passes where the stencil value is greater than it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StencilCompare {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Always,
    Never,
}

/// How a draw uses the stencil buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StencilMode {
    #[default]
    Off,
    /// Writes `value` where drawn; no colour is written (inside `stencil(...)`).
    Write { value: u8 },
    /// Draws only where the stencil passes the comparison.
    Test { compare: StencilCompare, value: u8 },
}

/// A scissor rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScissorRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// One vertex as the GPU reads it.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 2],
    pub texture_coordinate: [f32; 2],
    pub color: [f32; 4],
}

/// One batched draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCommand {
    Triangles {
        texture: TextureId,
        program: FragmentProgram,
        stencil: StencilMode,
        scissor: Option<ScissorRect>,
        first_vertex: u32,
        vertex_count: u32,
    },
    /// Resets the whole stencil buffer to 0 (LÖVE clears it before each `stencil` call).
    ClearStencil,
}

#[derive(Debug, Clone, Copy)]
struct State {
    color: Color,
    line_width: f32,
    smooth_lines: bool,
    transform: Transform,
}

/// Records one frame of drawing.
#[derive(Debug, Clone)]
pub struct DrawList {
    vertices: Vec<Vertex>,
    commands: Vec<DrawCommand>,
    state: State,
    saved_states: Vec<State>,
    scissor: Option<ScissorRect>,
    program: FragmentProgram,
    stencil_test: StencilMode,
    /// Set while recording the shapes of a `stencil(...)` call.
    writing_stencil: Option<u8>,
}

impl Default for DrawList {
    fn default() -> Self {
        DrawList::new()
    }
}

impl DrawList {
    pub fn new() -> DrawList {
        DrawList {
            vertices: Vec::new(),
            commands: Vec::new(),
            state: State { color: Color::WHITE, line_width: 1.0, smooth_lines: true, transform: Transform::IDENTITY },
            saved_states: Vec::new(),
            scissor: None,
            program: FragmentProgram::Plain,
            stencil_test: StencilMode::Off,
            writing_stencil: None,
        }
    }

    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    pub fn commands(&self) -> &[DrawCommand] {
        &self.commands
    }

    // -- State ---------------------------------------------------------------

    pub fn set_color(&mut self, color: Color) {
        self.state.color = color;
    }

    pub fn color(&self) -> Color {
        self.state.color
    }

    pub fn set_line_width(&mut self, width: f32) {
        self.state.line_width = width;
    }

    pub fn line_width(&self) -> f32 {
        self.state.line_width
    }

    /// LÖVE's `"smooth"` (feathered, the default) or `"rough"` line style.
    pub fn set_smooth_lines(&mut self, smooth: bool) {
        self.state.smooth_lines = smooth;
    }

    /// Saves the transform (and, as with `push("all")`, colour and line settings).
    pub fn push(&mut self) {
        self.saved_states.push(self.state);
    }

    pub fn pop(&mut self) {
        if let Some(state) = self.saved_states.pop() {
            self.state = state;
        }
    }

    pub fn origin(&mut self) {
        self.state.transform = Transform::IDENTITY;
    }

    pub fn translate(&mut self, x: f32, y: f32) {
        self.state.transform = self.state.transform.then(&Transform::translation(x, y));
    }

    pub fn scale(&mut self, x: f32, y: f32) {
        self.state.transform = self.state.transform.then(&Transform::scaling(x, y));
    }

    pub fn rotate(&mut self, angle: f32) {
        self.state.transform = self.state.transform.then(&Transform::rotation(angle));
    }

    pub fn transform(&self) -> Transform {
        self.state.transform
    }

    /// Scissor in physical pixels, like `love.graphics.setScissor` (transforms don't apply).
    pub fn set_scissor(&mut self, scissor: Option<ScissorRect>) {
        self.scissor = scissor.map(|rect| ScissorRect { width: rect.width.max(0), height: rect.height.max(0), ..rect });
    }

    pub fn scissor(&self) -> Option<ScissorRect> {
        self.scissor
    }

    pub fn set_program(&mut self, program: FragmentProgram) {
        self.program = program;
    }

    /// LÖVE's `stencil(draw, "replace", value)`: clears the stencil buffer, then the shapes drawn
    /// by `draw` write `value` without touching the colour.
    pub fn stencil(&mut self, value: u8, draw: impl FnOnce(&mut DrawList)) {
        self.commands.push(DrawCommand::ClearStencil);
        let previous = self.writing_stencil.replace(value);
        draw(self);
        self.writing_stencil = previous;
    }

    /// LÖVE's `setStencilTest(compare, value)`; `None` turns the test off.
    pub fn set_stencil_test(&mut self, test: Option<(StencilCompare, u8)>) {
        self.stencil_test = match test {
            Some((compare, value)) => StencilMode::Test { compare, value },
            None => StencilMode::Off,
        };
    }

    // -- Shapes --------------------------------------------------------------

    fn line_style(&self) -> LineStyle {
        LineStyle::new(self.state.line_width, self.state.smooth_lines)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rectangle(&mut self, mode: DrawMode, x: f32, y: f32, width: f32, height: f32, radius_x: f32, radius_y: f32) {
        let line = self.line_style();
        let triangles = tessellation::rectangle(mode, x, y, width, height, radius_x, radius_y, None, line);
        self.add_shape(mode, triangles);
    }

    /// A rounded rectangle with an explicit segment count.
    #[allow(clippy::too_many_arguments)]
    pub fn rectangle_with_points(
        &mut self,
        mode: DrawMode,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius_x: f32,
        radius_y: f32,
        points: i32,
    ) {
        let line = self.line_style();
        let triangles = tessellation::rectangle(mode, x, y, width, height, radius_x, radius_y, Some(points), line);
        self.add_shape(mode, triangles);
    }

    pub fn circle(&mut self, mode: DrawMode, x: f32, y: f32, radius: f32, points: Option<i32>) {
        let line = self.line_style();
        let triangles = tessellation::ellipse(mode, x, y, radius, radius, points, line);
        self.add_shape(mode, triangles);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn arc(
        &mut self,
        mode: DrawMode,
        arc_mode: ArcMode,
        x: f32,
        y: f32,
        radius: f32,
        angle1: f32,
        angle2: f32,
        points: Option<i32>,
    ) {
        let line = self.line_style();
        let triangles = tessellation::arc(mode, arc_mode, x, y, radius, angle1, angle2, points, line);
        self.add_shape(mode, triangles);
    }

    /// A line through `x1, y1, x2, y2, ...`.
    pub fn line(&mut self, coordinates: &[f32]) {
        let points = pairs_to_points(coordinates);
        let triangles = tessellation::polyline(&points, self.line_style().half_width, self.state.smooth_lines);
        self.add_triangles(TextureId::WHITE, &triangles, |_| [0.0, 0.0]);
    }

    pub fn polygon(&mut self, mode: DrawMode, coordinates: &[f32]) {
        let mut points = pairs_to_points(coordinates);
        if mode == DrawMode::Line
            && points.first() != points.last()
            && let Some(first) = points.first().copied()
        {
            points.push(first);
        }
        let triangles = match mode {
            DrawMode::Fill => tessellation::polygon(mode, &points, false, self.line_style()),
            DrawMode::Line => tessellation::polyline(&points, self.line_style().half_width, self.state.smooth_lines),
        };
        self.add_shape(mode, triangles);
    }

    fn add_shape(&mut self, _mode: DrawMode, triangles: Vec<ShapeVertex>) {
        self.add_triangles(TextureId::WHITE, &triangles, |_| [0.0, 0.0]);
    }

    // -- Textures --------------------------------------------------------------

    /// LÖVE's `draw(texture, [quad,] x, y, r, sx, sy, ox, oy)`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_texture(
        &mut self,
        texture: TextureInfo,
        quad: Option<Quad>,
        x: f32,
        y: f32,
        rotation: f32,
        scale_x: f32,
        scale_y: f32,
        origin_x: f32,
        origin_y: f32,
    ) {
        let quad = quad.unwrap_or(Quad { x: 0.0, y: 0.0, width: texture.width, height: texture.height });
        let local = Transform::translation(x, y)
            .then(&Transform::rotation(rotation))
            .then(&Transform::scaling(scale_x, scale_y))
            .then(&Transform::translation(-origin_x, -origin_y));
        let saved = self.state.transform;
        self.state.transform = saved.then(&local);
        let (u0, v0) = (quad.x / texture.width, quad.y / texture.height);
        let (u1, v1) = ((quad.x + quad.width) / texture.width, (quad.y + quad.height) / texture.height);
        self.add_textured_quad(texture.id, [0.0, 0.0, quad.width, quad.height], [u0, v0, u1, v1], self.state.color);
        self.state.transform = saved;
    }

    /// A rectangle (`[x, y, width, height]` in local units) showing part of a texture
    /// (`[u0, v0, u1, v1]`), tinted by `color`. Text glyphs use this.
    pub fn add_textured_quad(&mut self, texture: TextureId, rectangle: [f32; 4], uv: [f32; 4], color: Color) {
        let [x, y, width, height] = rectangle;
        let [u0, v0, u1, v1] = uv;
        let corners = [
            (Point::new(x, y), [u0, v0]),
            (Point::new(x + width, y), [u1, v0]),
            (Point::new(x + width, y + height), [u1, v1]),
            (Point::new(x, y + height), [u0, v1]),
        ];
        let saved_color = self.state.color;
        self.state.color = color;
        let order = [0, 1, 2, 0, 2, 3];
        let triangles: Vec<ShapeVertex> =
            order.iter().map(|&index| ShapeVertex { position: corners[index].0, alpha: 1.0 }).collect();
        let texture_coordinates: Vec<[f32; 2]> = order.iter().map(|&index| corners[index].1).collect();
        let mut next = 0;
        self.add_triangles(texture, &triangles, |_| {
            let coordinate = texture_coordinates[next];
            next += 1;
            coordinate
        });
        self.state.color = saved_color;
    }

    // -- Batching ------------------------------------------------------------

    fn add_triangles(
        &mut self,
        texture: TextureId,
        triangles: &[ShapeVertex],
        mut texture_coordinate: impl FnMut(usize) -> [f32; 2],
    ) {
        if triangles.is_empty() {
            return;
        }
        let color = self.state.color;
        let first_vertex = self.vertices.len() as u32;
        for (index, vertex) in triangles.iter().enumerate() {
            let position = self.state.transform.apply(vertex.position);
            self.vertices.push(Vertex {
                position: [position.x, position.y],
                texture_coordinate: texture_coordinate(index),
                color: [color.red, color.green, color.blue, color.alpha * vertex.alpha],
            });
        }
        let stencil = match self.writing_stencil {
            Some(value) => StencilMode::Write { value },
            None => self.stencil_test,
        };
        let vertex_count = triangles.len() as u32;
        if let Some(DrawCommand::Triangles {
            texture: last_texture,
            program: last_program,
            stencil: last_stencil,
            scissor: last_scissor,
            first_vertex: last_first,
            vertex_count: last_count,
        }) = self.commands.last_mut()
            && *last_texture == texture
            && *last_program == self.program
            && *last_stencil == stencil
            && *last_scissor == self.scissor
            && *last_first + *last_count == first_vertex
        {
            *last_count += vertex_count;
            return;
        }
        self.commands.push(DrawCommand::Triangles {
            texture,
            program: self.program,
            stencil,
            scissor: self.scissor,
            first_vertex,
            vertex_count,
        });
    }

    /// Adds everything `other` recorded after what this list holds (the UI's layers are recorded
    /// separately and drawn in order).
    pub fn append(&mut self, other: &DrawList) {
        let offset = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.commands.extend(other.commands.iter().map(|command| match *command {
            DrawCommand::Triangles { texture, program, stencil, scissor, first_vertex, vertex_count } => {
                DrawCommand::Triangles {
                    texture,
                    program,
                    stencil,
                    scissor,
                    first_vertex: first_vertex + offset,
                    vertex_count,
                }
            }
            DrawCommand::ClearStencil => DrawCommand::ClearStencil,
        }));
    }
}

fn pairs_to_points(coordinates: &[f32]) -> Vec<Point> {
    coordinates.chunks_exact(2).map(|pair| Point::new(pair[0], pair[1])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_compose_like_love() {
        let mut list = DrawList::new();
        list.scale(2.0, 2.0);
        list.translate(10.0, 5.0);
        let point = list.transform().apply(Point::new(1.0, 1.0));
        assert_eq!(point, Point::new(22.0, 12.0));
        list.push();
        list.rotate(std::f32::consts::FRAC_PI_2);
        let rotated = list.transform().apply(Point::new(1.0, 0.0));
        assert!((rotated.x - 20.0).abs() < 1e-5 && (rotated.y - 12.0).abs() < 1e-5);
        let back = list.transform().inverse().apply(rotated);
        assert!((back.x - 1.0).abs() < 1e-5 && back.y.abs() < 1e-5);
        list.pop();
        assert_eq!(list.transform().apply(Point::new(0.0, 0.0)), Point::new(20.0, 10.0));
    }

    #[test]
    fn consecutive_draws_with_the_same_state_share_a_command() {
        let mut list = DrawList::new();
        list.set_color(Color::rgba(1.0, 0.0, 0.0, 0.5));
        list.rectangle(DrawMode::Fill, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0);
        list.circle(DrawMode::Fill, 5.0, 5.0, 3.0, None);
        assert_eq!(list.commands().len(), 1);
        assert_eq!(list.vertices()[0].color, [1.0, 0.0, 0.0, 0.5]);
        list.set_scissor(Some(ScissorRect { x: 0, y: 0, width: 5, height: 5 }));
        list.rectangle(DrawMode::Fill, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0);
        list.set_program(FragmentProgram::Greyscale);
        list.rectangle(DrawMode::Fill, 0.0, 0.0, 10.0, 10.0, 0.0, 0.0);
        assert_eq!(list.commands().len(), 3);
    }

    #[test]
    fn stencil_writes_then_tests() {
        let mut list = DrawList::new();
        list.stencil(1, |list| list.rectangle(DrawMode::Fill, 0.0, 0.0, 10.0, 10.0, 2.0, 2.0));
        list.set_stencil_test(Some((StencilCompare::Greater, 0)));
        list.draw_texture(
            TextureInfo { id: TextureId(7), width: 4.0, height: 2.0 },
            None,
            1.0,
            1.0,
            0.0,
            2.0,
            2.0,
            0.0,
            0.0,
        );
        list.set_stencil_test(None);
        let commands = list.commands();
        assert_eq!(commands[0], DrawCommand::ClearStencil);
        assert!(matches!(commands[1], DrawCommand::Triangles { stencil: StencilMode::Write { value: 1 }, .. }));
        assert!(matches!(
            commands[2],
            DrawCommand::Triangles {
                texture: TextureId(7),
                stencil: StencilMode::Test { compare: StencilCompare::Greater, value: 0 },
                ..
            }
        ));
        let image_vertices = &list.vertices()[list.vertices().len() - 6..];
        assert_eq!(image_vertices[2].position, [9.0, 5.0]);
        assert_eq!(image_vertices[2].texture_coordinate, [1.0, 1.0]);
    }

    #[test]
    fn quads_pick_part_of_a_texture() {
        let mut list = DrawList::new();
        let texture = TextureInfo { id: TextureId(3), width: 100.0, height: 50.0 };
        list.draw_texture(
            texture,
            Some(Quad { x: 25.0, y: 0.0, width: 50.0, height: 50.0 }),
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            0.0,
            0.0,
        );
        let vertices = list.vertices();
        assert_eq!(vertices[0].texture_coordinate, [0.25, 0.0]);
        assert_eq!(vertices[2].texture_coordinate, [0.75, 1.0]);
        assert_eq!(vertices[2].position, [50.0, 50.0]);
    }

    #[test]
    fn rough_lines_have_no_feather() {
        let mut list = DrawList::new();
        list.set_smooth_lines(false);
        list.line(&[0.0, 0.0, 10.0, 0.0]);
        assert_eq!(list.vertices().len(), 6);
        assert!(list.vertices().iter().all(|vertex| vertex.color[3] == 1.0));
    }
}
