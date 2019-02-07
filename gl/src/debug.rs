// pathfinder/gl/src/debug.rs
//
// Copyright © 2019 The Pathfinder Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! A debug overlay.
//!
//! We don't render the demo UI text using Pathfinder itself so that we can use the debug UI to
//! debug Pathfinder if it's totally busted.
//!
//! The debug font atlas was generated using: https://evanw.github.io/font-texture-generator/

use crate::device::{Buffer, BufferTarget, BufferUploadMode, Program, Texture, Uniform, VertexAttr};
use euclid::Size2D;
use gl::types::{GLfloat, GLint, GLsizei, GLuint};
use gl;
use pathfinder_geometry::basic::point::Point2DI32;
use pathfinder_geometry::basic::rect::RectI32;
use pathfinder_renderer::paint::ColorU;
use serde_json;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufReader;
use std::ptr;
use std::time::Duration;

const SAMPLE_BUFFER_SIZE: usize = 60;

const DEBUG_TEXTURE_VERTEX_SIZE: GLint = 8;
const DEBUG_SOLID_VERTEX_SIZE:   GLint = 4;

pub const PADDING: i32 = 12;
pub const BUTTON_WIDTH: i32 = PADDING * 2 + ICON_SIZE;
pub const BUTTON_HEIGHT: i32 = PADDING * 2 + ICON_SIZE;
pub const BUTTON_TEXT_OFFSET: i32 = PADDING + 36;

pub static TEXT_COLOR:   ColorU = ColorU { r: 255, g: 255, b: 255, a: 255      };
pub static WINDOW_COLOR: ColorU = ColorU { r: 30,  g: 30,  b: 30,  a: 255 - 30 };

const PERF_WINDOW_WIDTH: i32 = 300;
const PERF_WINDOW_HEIGHT: i32 = LINE_HEIGHT * 2 + PADDING + 2;
const FONT_ASCENT: i32 = 28;
const LINE_HEIGHT: i32 = 42;
const ICON_SIZE: i32 = 48;

static INVERTED_TEXT_COLOR: ColorU = ColorU { r: 0,   g: 0,   b: 0,   a: 255      };

static JSON_PATH: &'static str = "resources/debug-font.json";

static FONT_PNG_NAME: &'static str = "debug-font";

static QUAD_INDICES: [u32; 6] = [0, 1, 3, 1, 2, 3];

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct DebugFont {
    name: String,
    size: i32,
    bold: bool,
    italic: bool,
    width: u32,
    height: u32,
    characters: HashMap<char, DebugCharacter>,
}

#[derive(Deserialize)]
struct DebugCharacter {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    #[serde(rename = "originX")]
    origin_x: i32,
    #[serde(rename = "originY")]
    origin_y: i32,
    advance: i32,
}

impl DebugFont {
    fn load() -> DebugFont {
        serde_json::from_reader(BufReader::new(File::open(JSON_PATH).unwrap())).unwrap()
    }
}

pub struct DebugUI {
    framebuffer_size: Size2D<u32>,

    texture_program: DebugTextureProgram,
    texture_vertex_array: DebugTextureVertexArray,
    font: DebugFont,
    solid_program: DebugSolidProgram,
    solid_vertex_array: DebugSolidVertexArray,
    font_texture: Texture,

    cpu_samples: SampleBuffer,
    gpu_samples: SampleBuffer,
}

impl DebugUI {
    pub fn new(framebuffer_size: &Size2D<u32>) -> DebugUI {
        let texture_program = DebugTextureProgram::new();
        let texture_vertex_array = DebugTextureVertexArray::new(&texture_program);
        let font = DebugFont::load();

        let solid_program = DebugSolidProgram::new();
        let solid_vertex_array = DebugSolidVertexArray::new(&solid_program);
        solid_vertex_array.index_buffer.upload(&QUAD_INDICES,
                                               BufferTarget::Index,
                                               BufferUploadMode::Static);

        let font_texture = Texture::from_png(FONT_PNG_NAME);

        DebugUI {
            framebuffer_size: *framebuffer_size,
            texture_program,
            texture_vertex_array,
            font,
            solid_program,
            solid_vertex_array,
            font_texture,
            cpu_samples: SampleBuffer::new(),
            gpu_samples: SampleBuffer::new(),
        }
    }

    pub fn framebuffer_size(&self) -> Size2D<u32> {
        self.framebuffer_size
    }

    pub fn set_framebuffer_size(&mut self, window_size: &Size2D<u32>) {
        self.framebuffer_size = *window_size;
    }

    pub fn add_sample(&mut self, tile_time: Duration, rendering_time: Option<Duration>) {
        self.cpu_samples.push(tile_time);
        if let Some(rendering_time) = rendering_time {
            self.gpu_samples.push(rendering_time);
        }
    }

    pub fn draw(&self) {
        // Draw performance window.
        let bottom = self.framebuffer_size.height as i32 - PADDING;
        let window_rect = RectI32::new(
            Point2DI32::new(self.framebuffer_size.width as i32 - PADDING - PERF_WINDOW_WIDTH,
                            bottom - PERF_WINDOW_HEIGHT),
            Point2DI32::new(PERF_WINDOW_WIDTH, PERF_WINDOW_HEIGHT));
        self.draw_solid_rect(window_rect, WINDOW_COLOR);
        self.draw_text(&format!("CPU: {:.3} ms", self.cpu_samples.mean_ms()),
                       Point2DI32::new(window_rect.min_x() + PADDING,
                                       window_rect.min_y() + PADDING + FONT_ASCENT),
                       false);
        self.draw_text(&format!("GPU: {:.3} ms", self.gpu_samples.mean_ms()),
                        Point2DI32::new(
                            window_rect.min_x() + PADDING,
                            window_rect.min_y() + PADDING + FONT_ASCENT + LINE_HEIGHT),
                       false);
    }

    pub fn draw_solid_rect(&self, rect: RectI32, color: ColorU) {
        self.draw_rect(rect, color, true);
    }

    pub fn draw_rect_outline(&self, rect: RectI32, color: ColorU) {
        self.draw_rect(rect, color, false);
    }

    fn draw_rect(&self, rect: RectI32, color: ColorU, filled: bool) {
        let vertex_data = [
            DebugSolidVertex::new(rect.origin()),
            DebugSolidVertex::new(rect.upper_right()),
            DebugSolidVertex::new(rect.lower_right()),
            DebugSolidVertex::new(rect.lower_left()),
        ];
        self.solid_vertex_array
            .vertex_buffer
            .upload(&vertex_data, BufferTarget::Vertex, BufferUploadMode::Dynamic);

        unsafe {
            gl::BindVertexArray(self.solid_vertex_array.gl_vertex_array);
            gl::UseProgram(self.solid_program.program.gl_program);
            gl::Uniform2f(self.solid_program.framebuffer_size_uniform.location,
                          self.framebuffer_size.width as GLfloat,
                          self.framebuffer_size.height as GLfloat);
            set_color_uniform(&self.solid_program.color_uniform, color);
            gl::BlendEquation(gl::FUNC_ADD);
            gl::BlendFunc(gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);
            if filled {
                gl::DrawElements(gl::TRIANGLES, 6, gl::UNSIGNED_INT, ptr::null());
            } else {
                gl::DrawArrays(gl::LINE_LOOP, 0, 4);
            }
            gl::Disable(gl::BLEND);
        }
    }

    pub fn draw_text(&self, string: &str, origin: Point2DI32, invert: bool) {
        let mut next = origin;
        let char_count = string.chars().count();
        let mut vertex_data = Vec::with_capacity(char_count * 4);
        let mut index_data = Vec::with_capacity(char_count * 6);
        for mut character in string.chars() {
            if !self.font.characters.contains_key(&character) {
                character = '?';
            }

            let info = &self.font.characters[&character];
            let position_rect =
                RectI32::new(Point2DI32::new(next.x() - info.origin_x, next.y() - info.origin_y),
                             Point2DI32::new(info.width as i32, info.height as i32));
            let tex_coord_rect = RectI32::new(Point2DI32::new(info.x, info.y),
                                              Point2DI32::new(info.width, info.height));
            let first_vertex_index = vertex_data.len();
            vertex_data.extend_from_slice(&[
                DebugTextureVertex::new(position_rect.origin(),      tex_coord_rect.origin()),
                DebugTextureVertex::new(position_rect.upper_right(), tex_coord_rect.upper_right()),
                DebugTextureVertex::new(position_rect.lower_right(), tex_coord_rect.lower_right()),
                DebugTextureVertex::new(position_rect.lower_left(),  tex_coord_rect.lower_left()),
            ]);
            index_data.extend(QUAD_INDICES.iter().map(|&i| i + first_vertex_index as u32));

            let next_x = next.x() + info.advance;
            next.set_x(next_x);
        }

        let color = if invert { INVERTED_TEXT_COLOR } else { TEXT_COLOR };
        self.draw_texture_with_vertex_data(&vertex_data, &index_data, &self.font_texture, color);
    }

    pub fn draw_texture(&self, origin: Point2DI32, texture: &Texture, color: ColorU) {
        let size = Point2DI32::new(texture.size.width as i32, texture.size.height as i32);
        let position_rect = RectI32::new(origin, size);
        let tex_coord_rect = RectI32::new(Point2DI32::default(), size);
        let vertex_data = [
            DebugTextureVertex::new(position_rect.origin(),      tex_coord_rect.origin()),
            DebugTextureVertex::new(position_rect.upper_right(), tex_coord_rect.upper_right()),
            DebugTextureVertex::new(position_rect.lower_right(), tex_coord_rect.lower_right()),
            DebugTextureVertex::new(position_rect.lower_left(),  tex_coord_rect.lower_left()),
        ];

        self.draw_texture_with_vertex_data(&vertex_data, &QUAD_INDICES, texture, color);
    }

    pub fn measure_text(&self, string: &str) -> i32 {
        let mut next = 0;
        for mut character in string.chars() {
            if !self.font.characters.contains_key(&character) {
                character = '?';
            }

            let info = &self.font.characters[&character];
            next += info.advance;
        }
        next
    }

    fn draw_texture_with_vertex_data(&self,
                                     vertex_data: &[DebugTextureVertex],
                                     index_data: &[u32],
                                     texture: &Texture,
                                     color: ColorU) {
        self.texture_vertex_array
            .vertex_buffer
            .upload(&vertex_data, BufferTarget::Vertex, BufferUploadMode::Dynamic);
        self.texture_vertex_array
            .index_buffer
            .upload(&index_data, BufferTarget::Index, BufferUploadMode::Dynamic);

        unsafe {
            gl::BindVertexArray(self.texture_vertex_array.gl_vertex_array);
            gl::UseProgram(self.texture_program.program.gl_program);
            gl::Uniform2f(self.texture_program.framebuffer_size_uniform.location,
                          self.framebuffer_size.width as GLfloat,
                          self.framebuffer_size.height as GLfloat);
            gl::Uniform2f(self.texture_program.texture_size_uniform.location,
                          texture.size.width as GLfloat,
                          texture.size.height as GLfloat);
            set_color_uniform(&self.texture_program.color_uniform, color);
            texture.bind(0);
            gl::Uniform1i(self.texture_program.texture_uniform.location, 0);
            gl::BlendEquation(gl::FUNC_ADD);
            gl::BlendFunc(gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
            gl::Enable(gl::BLEND);
            gl::DrawElements(gl::TRIANGLES,
                             index_data.len() as GLsizei,
                             gl::UNSIGNED_INT,
                             ptr::null());
            gl::Disable(gl::BLEND);
        }
    }
}

struct DebugTextureVertexArray {
    gl_vertex_array: GLuint,
    vertex_buffer: Buffer,
    index_buffer: Buffer,
}

impl DebugTextureVertexArray {
    fn new(debug_texture_program: &DebugTextureProgram) -> DebugTextureVertexArray {
        let vertex_buffer = Buffer::new();
        let index_buffer = Buffer::new();
        let mut gl_vertex_array = 0;
        unsafe {
            let position_attr = VertexAttr::new(&debug_texture_program.program, "Position");
            let tex_coord_attr = VertexAttr::new(&debug_texture_program.program, "TexCoord");

            gl::GenVertexArrays(1, &mut gl_vertex_array);
            gl::BindVertexArray(gl_vertex_array);
            gl::UseProgram(debug_texture_program.program.gl_program);
            gl::BindBuffer(gl::ARRAY_BUFFER, vertex_buffer.gl_buffer);
            position_attr.configure_float(2,
                                          gl::UNSIGNED_SHORT,
                                          false,
                                          DEBUG_TEXTURE_VERTEX_SIZE,
                                          0,
                                          0);
            tex_coord_attr.configure_float(2,
                                           gl::UNSIGNED_SHORT,
                                           false,
                                           DEBUG_TEXTURE_VERTEX_SIZE,
                                           4,
                                           0);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, index_buffer.gl_buffer);
        }

        DebugTextureVertexArray { gl_vertex_array, vertex_buffer, index_buffer }
    }
}

impl Drop for DebugTextureVertexArray {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            gl::DeleteVertexArrays(1, &mut self.gl_vertex_array);
        }
    }
}

struct DebugSolidVertexArray {
    gl_vertex_array: GLuint,
    vertex_buffer: Buffer,
    index_buffer: Buffer,
}

impl DebugSolidVertexArray {
    fn new(debug_solid_program: &DebugSolidProgram) -> DebugSolidVertexArray {
        let vertex_buffer = Buffer::new();
        let index_buffer = Buffer::new();
        let mut gl_vertex_array = 0;
        unsafe {
            let position_attr = VertexAttr::new(&debug_solid_program.program, "Position");

            gl::GenVertexArrays(1, &mut gl_vertex_array);
            gl::BindVertexArray(gl_vertex_array);
            gl::UseProgram(debug_solid_program.program.gl_program);
            gl::BindBuffer(gl::ARRAY_BUFFER, vertex_buffer.gl_buffer);
            position_attr.configure_float(2,
                                          gl::UNSIGNED_SHORT,
                                          false,
                                          DEBUG_SOLID_VERTEX_SIZE,
                                          0,
                                          0);
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, index_buffer.gl_buffer);
        }

        DebugSolidVertexArray { gl_vertex_array, vertex_buffer, index_buffer }
    }
}

impl Drop for DebugSolidVertexArray {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            gl::DeleteVertexArrays(1, &mut self.gl_vertex_array);
        }
    }
}

struct DebugTextureProgram {
    program: Program,
    framebuffer_size_uniform: Uniform,
    texture_size_uniform: Uniform,
    texture_uniform: Uniform,
    color_uniform: Uniform,
}

impl DebugTextureProgram {
    fn new() -> DebugTextureProgram {
        let program = Program::new("debug_texture");
        let framebuffer_size_uniform = Uniform::new(&program, "FramebufferSize");
        let texture_size_uniform = Uniform::new(&program, "TextureSize");
        let texture_uniform = Uniform::new(&program, "Texture");
        let color_uniform = Uniform::new(&program, "Color");
        DebugTextureProgram {
            program,
            framebuffer_size_uniform,
            texture_size_uniform,
            texture_uniform,
            color_uniform,
        }
    }
}

struct DebugSolidProgram {
    program: Program,
    framebuffer_size_uniform: Uniform,
    color_uniform: Uniform,
}

impl DebugSolidProgram {
    fn new() -> DebugSolidProgram {
        let program = Program::new("debug_solid");
        let framebuffer_size_uniform = Uniform::new(&program, "FramebufferSize");
        let color_uniform = Uniform::new(&program, "Color");
        DebugSolidProgram { program, framebuffer_size_uniform, color_uniform }
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct DebugTextureVertex {
    position_x: i16,
    position_y: i16,
    tex_coord_x: u16,
    tex_coord_y: u16,
}

impl DebugTextureVertex {
    fn new(position: Point2DI32, tex_coord: Point2DI32) -> DebugTextureVertex {
        DebugTextureVertex {
            position_x: position.x() as i16,
            position_y: position.y() as i16,
            tex_coord_x: tex_coord.x() as u16,
            tex_coord_y: tex_coord.y() as u16,
        }
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct DebugSolidVertex {
    position_x: i16,
    position_y: i16,
}

impl DebugSolidVertex {
    fn new(position: Point2DI32) -> DebugSolidVertex {
        DebugSolidVertex { position_x: position.x() as i16, position_y: position.y() as i16 }
    }
}

struct SampleBuffer {
    samples: VecDeque<Duration>,
}

impl SampleBuffer {
    fn new() -> SampleBuffer {
        SampleBuffer { samples: VecDeque::with_capacity(SAMPLE_BUFFER_SIZE) }
    }

    fn push(&mut self, time: Duration) {
        self.samples.push_back(time);
        while self.samples.len() > SAMPLE_BUFFER_SIZE {
            self.samples.pop_front();
        }
    }

    fn mean_ms(&self) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }

        let mut ms = 0.0;
        for time in &self.samples {
            ms += time.as_secs() as f64 * 1000.0 + time.subsec_nanos() as f64 / 1000000.0;
        }
        ms / self.samples.len() as f64
    }
}

fn set_color_uniform(uniform: &Uniform, color: ColorU) {
    unsafe {
        gl::Uniform4f(uniform.location,
                      color.r as f32 * (1.0 / 255.0),
                      color.g as f32 * (1.0 / 255.0),
                      color.b as f32 * (1.0 / 255.0),
                      color.a as f32 * (1.0 / 255.0));
    }
}
