use std::collections::HashMap;
use std::mem;

use ahash::RandomState;
use crossfont::Metrics;
use log::info;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::cell::Flags;

use crate::display::color::Rgb;
use crate::display::content::RenderableCell;
use crate::display::SizeInfo;
use crate::gl;
use crate::gl::types::*;
use crate::renderer::shader::{ShaderError, ShaderProgram, ShaderVersion};
use crate::renderer::{self, cstr};

#[derive(Debug, Copy, Clone)]
pub struct RenderRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Rgb,
    pub alpha: f32,
    pub kind: RectKind,
}

impl RenderRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32, color: Rgb, alpha: f32) -> Self {
        RenderRect { kind: RectKind::Underline, x, y, width, height, color, alpha }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RenderLine {
    pub start: Point<usize>,
    pub end: Point<usize>,
    pub color: Rgb,
}

// NOTE: These flags must be in sync with their usage in the rect.*.glsl shaders.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RectKind {
    Underline = 0,
    Undercurl = 1,
    UnderDotted = 2,
    UnderDashed = 3,
    RoundedBg = 4,
    NumKinds = 5,
}

impl RenderLine {
    pub fn rects(&self, flag: Flags, metrics: &Metrics, size: &SizeInfo) -> Vec<RenderRect> {
        let mut rects = Vec::new();

        let mut start = self.start;
        while start.line < self.end.line {
            let end = Point::new(start.line, size.last_column());
            Self::push_rects(&mut rects, metrics, size, flag, start, end, self.color);
            start = Point::new(start.line + 1, Column(0));
        }
        Self::push_rects(&mut rects, metrics, size, flag, start, self.end, self.color);

        rects
    }

    /// Push all rects required to draw the cell's line.
    fn push_rects(
        rects: &mut Vec<RenderRect>,
        metrics: &Metrics,
        size: &SizeInfo,
        flag: Flags,
        start: Point<usize>,
        end: Point<usize>,
        color: Rgb,
    ) {
        let (position, thickness, ty) = match flag {
            Flags::DOUBLE_UNDERLINE => {
                // Position underlines so each one has 50% of descent available.
                let top_pos = 0.25 * metrics.descent;
                let bottom_pos = 0.75 * metrics.descent;

                rects.push(Self::create_rect(
                    size,
                    metrics.descent,
                    start,
                    end,
                    top_pos,
                    metrics.underline_thickness,
                    color,
                ));

                (bottom_pos, metrics.underline_thickness, RectKind::Underline)
            },
            // Make undercurl occupy the entire descent area.
            Flags::UNDERCURL => (metrics.descent, metrics.descent.abs(), RectKind::Undercurl),
            Flags::UNDERLINE => {
                (metrics.underline_position, metrics.underline_thickness, RectKind::Underline)
            },
            // Make dotted occupy the entire descent area.
            Flags::DOTTED_UNDERLINE => {
                (metrics.descent, metrics.descent.abs(), RectKind::UnderDotted)
            },
            Flags::DASHED_UNDERLINE => {
                (metrics.underline_position, metrics.underline_thickness, RectKind::UnderDashed)
            },
            Flags::STRIKEOUT => {
                (metrics.strikeout_position, metrics.strikeout_thickness, RectKind::Underline)
            },
            Flags::ROUNDED_BACKGROUND => (metrics.descent, size.cell_height(), RectKind::RoundedBg),
            _ => unimplemented!("Invalid flag for cell line drawing specified"),
        };

        let mut rect =
            Self::create_rect(size, metrics.descent, start, end, position, thickness, color);
        rect.kind = ty;
        rects.push(rect);
    }

    /// Create a line's rect at a position relative to the baseline.
    fn create_rect(
        size: &SizeInfo,
        descent: f32,
        start: Point<usize>,
        end: Point<usize>,
        position: f32,
        mut thickness: f32,
        color: Rgb,
    ) -> RenderRect {
        let start_x = start.column.0 as f32 * size.cell_width();
        let end_x = (end.column.0 + 1) as f32 * size.cell_width();
        let width = end_x - start_x;

        // Make sure lines are always visible.
        thickness = thickness.max(1.);

        let line_bottom = (start.line as f32 + 1.) * size.cell_height();
        let baseline = line_bottom + descent;

        let mut y = (baseline - position - thickness / 2.).round();
        let max_y = line_bottom - thickness;
        if y > max_y {
            y = max_y;
        }

        RenderRect::new(
            start_x + size.padding_x(),
            y + size.padding_y(),
            width,
            thickness,
            color,
            1.,
        )
    }
}

/// Lines for underline and strikeout.
#[derive(Default)]
pub struct RenderLines {
    inner: HashMap<Flags, Vec<RenderLine>, RandomState>,
}

impl RenderLines {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn rects(&self, metrics: &Metrics, size: &SizeInfo) -> Vec<RenderRect> {
        self.inner
            .iter()
            .flat_map(|(flag, lines)| {
                lines.iter().flat_map(move |line| line.rects(*flag, metrics, size))
            })
            .collect()
    }

    /// Update the stored lines with the next cell info.
    #[inline]
    pub fn update(&mut self, cell: &RenderableCell) {
        self.update_flag(cell, Flags::UNDERLINE);
        self.update_flag(cell, Flags::DOUBLE_UNDERLINE);
        self.update_flag(cell, Flags::STRIKEOUT);
        self.update_flag(cell, Flags::UNDERCURL);
        self.update_flag(cell, Flags::DOTTED_UNDERLINE);
        self.update_flag(cell, Flags::DASHED_UNDERLINE);
        self.update_flag(cell, Flags::ROUNDED_BACKGROUND);
    }

    /// Update the lines for a specific flag.
    fn update_flag(&mut self, cell: &RenderableCell, flag: Flags) {
        if !cell.flags.contains(flag) {
            return;
        }

        // The underline color escape does not apply to strikeout.
        let color = if flag.contains(Flags::STRIKEOUT) { cell.fg } else { cell.underline };

        // Include wide char spacer if the current cell is a wide char.
        let mut end = cell.point;
        if cell.flags.contains(Flags::WIDE_CHAR) {
            end.column += 1;
        }

        // Check if there's an active line.
        if let Some(line) = self.inner.get_mut(&flag).and_then(|lines| lines.last_mut()) {
            if color == line.color
                && cell.point.column == line.end.column + 1
                && cell.point.line == line.end.line
            {
                // Update the length of the line.
                line.end = end;
                return;
            }
        }

        // Start new line if there currently is none.
        let line = RenderLine { start: cell.point, end, color };
        match self.inner.get_mut(&flag) {
            Some(lines) => lines.push(line),
            None => {
                self.inner.insert(flag, vec![line]);
            },
        }
    }
}

/// Shader sources for rect rendering program.
static RECT_SHADER_F: &str = include_str!("../../res/rect.f.glsl");
static RECT_SHADER_V: &str = include_str!("../../res/rect.v.glsl");

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Vertex {
    // Normalized screen coordinates.
    x: f32,
    y: f32,

    // Color.
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

#[derive(Debug)]
pub struct RectRenderer {
    // GL buffer objects.
    vao: GLuint,
    vbo: GLuint,

    programs: [RectShaderProgram; RectKind::NumKinds as usize],
    vertices: [Vec<Vertex>; RectKind::NumKinds as usize],
}

impl RectRenderer {
    pub fn new(shader_version: ShaderVersion) -> Result<Self, renderer::Error> {
        let mut vao: GLuint = 0;
        let mut vbo: GLuint = 0;

        let under_line_program = RectShaderProgram::new(shader_version, RectKind::Underline)?;
        let under_curl_program = RectShaderProgram::new(shader_version, RectKind::Undercurl)?;
        // This shader has way more ALU operations than other rect shaders, so use a fallback
        // to underline just for it when we can't compile it.
        let under_dotted_program = match RectShaderProgram::new(shader_version, RectKind::UnderDotted)
        {
            Ok(under_dotted_program) => under_dotted_program,
            Err(err) => {
                info!("Error compiling dotted shader: {err}\n  falling back to underline");
                RectShaderProgram::new(shader_version, RectKind::Underline)?
            },
        };
        let under_dashed_program = RectShaderProgram::new(shader_version, RectKind::UnderDashed)?;
        let rounded_background_program = RectShaderProgram::new(shader_version, RectKind::RoundedBg)?;

        unsafe {
            // Allocate buffers.
            gl::GenVertexArrays(1, &mut vao);
            gl::GenBuffers(1, &mut vbo);

            gl::BindVertexArray(vao);

            // VBO binding is not part of VAO itself, but VBO binding is stored in attributes.
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);

            let mut attribute_offset = 0;

            // Position.
            gl::VertexAttribPointer(
                0,
                2,
                gl::FLOAT,
                gl::FALSE,
                mem::size_of::<Vertex>() as i32,
                attribute_offset as *const _,
            );
            gl::EnableVertexAttribArray(0);
            attribute_offset += mem::size_of::<f32>() * 2;

            // Color.
            gl::VertexAttribPointer(
                1,
                4,
                gl::UNSIGNED_BYTE,
                gl::TRUE,
                mem::size_of::<Vertex>() as i32,
                attribute_offset as *const _,
            );
            gl::EnableVertexAttribArray(1);

            // Reset buffer bindings.
            gl::BindVertexArray(0);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
        }

        let programs = [
            under_line_program,
            under_curl_program,
            under_dotted_program,
            under_dashed_program,
            rounded_background_program,
        ];
        Ok(Self { vao, vbo, programs, vertices: Default::default() })
    }

    pub fn draw(&mut self, size_info: &SizeInfo, metrics: &Metrics, rects: Vec<RenderRect>) {
        unsafe {
            // Bind VAO to enable vertex attribute slots.
            gl::BindVertexArray(self.vao);

            // Bind VBO only once for buffer data upload only.
            gl::BindBuffer(gl::ARRAY_BUFFER, self.vbo);
        }

        let center_x = size_info.width() / 2.;
        let center_y = size_info.height() / 2.;

        // Build rect vertices vector.
        self.vertices.iter_mut().for_each(|vertices| vertices.clear());
        for rect in &rects {
            Self::add_rect(&mut self.vertices[rect.kind as usize], center_x, center_y, rect);
        }

        unsafe {
            // We iterate in reverse order to draw plain rects at the end, since we want visual
            // bell or damage rects be above the lines.
            for rect_kind in (RectKind::Underline as u8..RectKind::NumKinds as u8).rev() {
                let vertices = &mut self.vertices[rect_kind as usize];
                if vertices.is_empty() {
                    continue;
                }

                let program = &self.programs[rect_kind as usize];
                gl::UseProgram(program.id());
                program.update_uniforms(size_info, metrics);

                // Upload accumulated undercurl vertices.
                gl::BufferData(
                    gl::ARRAY_BUFFER,
                    (vertices.len() * mem::size_of::<Vertex>()) as isize,
                    vertices.as_ptr() as *const _,
                    gl::STREAM_DRAW,
                );

                // Draw all vertices as list of triangles.
                gl::DrawArrays(gl::TRIANGLES, 0, vertices.len() as i32);
            }

            // Disable program.
            gl::UseProgram(0);

            // Reset buffer bindings to nothing.
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::BindVertexArray(0);
        }
    }

    fn add_rect(vertices: &mut Vec<Vertex>, center_x: f32, center_y: f32, rect: &RenderRect) {
        // Calculate rectangle vertices positions in normalized device coordinates.
        // NDC range from -1 to +1, with Y pointing up.
        let x = rect.x / center_x - 1.0;
        let y = -rect.y / center_y + 1.0;
        let width = rect.width / center_x;
        let height = rect.height / center_y;
        let (r, g, b) = rect.color.as_tuple();
        let a = (rect.alpha * 255.) as u8;

        // Make quad vertices.
        let quad = [
            Vertex { x, y, r, g, b, a },
            Vertex { x, y: y - height, r, g, b, a },
            Vertex { x: x + width, y, r, g, b, a },
            Vertex { x: x + width, y: y - height, r, g, b, a },
        ];

        // Append the vertices to form two triangles.
        vertices.push(quad[0]);
        vertices.push(quad[1]);
        vertices.push(quad[2]);
        vertices.push(quad[2]);
        vertices.push(quad[3]);
        vertices.push(quad[1]);
    }
}

impl Drop for RectRenderer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteBuffers(1, &self.vbo);
            gl::DeleteVertexArrays(1, &self.vao);
        }
    }
}

/// Rectangle drawing program.
#[derive(Debug)]
pub struct RectShaderProgram {
    /// Shader program.
    program: ShaderProgram,

    /// Cell width.
    u_cell_width: Option<GLint>,

    /// Cell height.
    u_cell_height: Option<GLint>,

    /// Terminal padding.
    u_padding_x: Option<GLint>,

    /// A padding from the bottom of the screen to viewport.
    u_padding_y: Option<GLint>,

    /// Underline position.
    u_underline_position: Option<GLint>,

    /// Underline thickness.
    u_underline_thickness: Option<GLint>,

    /// Undercurl position.
    u_undercurl_position: Option<GLint>,
}

impl RectShaderProgram {
    pub fn new(shader_version: ShaderVersion, kind: RectKind) -> Result<Self, ShaderError> {
        // XXX: This must be in-sync with fragment shader defines.
        let header = match kind {
            RectKind::RoundedBg => Some("#define DRAW_ROUNDED_BACKGROUND\n"),
             RectKind::Undercurl => Some("#define DRAW_UNDER_CURL\n"),
            RectKind::UnderDotted => Some("#define DRAW_UNDER_DOTTED\n"),
            RectKind::UnderDashed => Some("#define DRAW_UNDER_DASHED\n"),
            _ => None,
        };
        let program = ShaderProgram::new(shader_version, header, RECT_SHADER_V, RECT_SHADER_F)?;

        Ok(Self {
            u_cell_width: program.get_uniform_location(cstr!("cellWidth")).ok(),
            u_cell_height: program.get_uniform_location(cstr!("cellHeight")).ok(),
            u_padding_x: program.get_uniform_location(cstr!("paddingX")).ok(),
            u_padding_y: program.get_uniform_location(cstr!("paddingY")).ok(),
            u_underline_position: program.get_uniform_location(cstr!("underlinePosition")).ok(),
            u_underline_thickness: program.get_uniform_location(cstr!("underlineThickness")).ok(),
            u_undercurl_position: program.get_uniform_location(cstr!("undercurlPosition")).ok(),
            program,
        })
    }

    fn id(&self) -> GLuint {
        self.program.id()
    }

    pub fn update_uniforms(&self, size_info: &SizeInfo, metrics: &Metrics) {
        let position = (0.5 * metrics.descent).abs();
        let underline_position = metrics.descent.abs() - metrics.underline_position.abs();

        let viewport_height = size_info.height() - size_info.padding_y();
        let padding_y = viewport_height
            - (viewport_height / size_info.cell_height()).floor() * size_info.cell_height();

        unsafe {
            if let Some(u_cell_width) = self.u_cell_width {
                gl::Uniform1f(u_cell_width, size_info.cell_width());
            }
            if let Some(u_cell_height) = self.u_cell_height {
                gl::Uniform1f(u_cell_height, size_info.cell_height());
            }
            if let Some(u_padding_y) = self.u_padding_y {
                gl::Uniform1f(u_padding_y, padding_y);
            }
            if let Some(u_padding_x) = self.u_padding_x {
                gl::Uniform1f(u_padding_x, size_info.padding_x());
            }
            if let Some(u_underline_position) = self.u_underline_position {
                gl::Uniform1f(u_underline_position, underline_position);
            }
            if let Some(u_underline_thickness) = self.u_underline_thickness {
                gl::Uniform1f(u_underline_thickness, metrics.underline_thickness);
            }
            if let Some(u_undercurl_position) = self.u_undercurl_position {
                gl::Uniform1f(u_undercurl_position, position);
            }
        }
    }
}
