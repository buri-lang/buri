//! What an image source paints: a PNG reader, an SVG subset, and the rule that
//! sizes both.
//!
//! [`read`] is the whole entry: a `data:` URI in, a [`Picture`] out, and
//! [`artwork`] is the same for an icon's SVG, which the scene carries whole.
//! `paint.rs`'s `picture` calls it and paints what comes back, and everything
//! it answers `None` for is a placeholder box.
//!
//! **Nothing is fetched and nothing is opened**, which is `paint.rs`'s rule and
//! not this file's: a snapshot that reached the network would answer a
//! different picture on a different day. So an `http` URL and a path are
//! placeholders no matter what is behind them, and the only source that can be
//! read is the one the scene document carries whole.
//!
//! # PNG
//!
//! All of the still image format: colour types 0, 2, 3, 4 and 6, bit depths 1,
//! 2 and 4 for grey and palette and 8 and 16 for everything, `tRNS`
//! transparency in all three of its forms, every row filter, and a full RFC
//! 1951 inflate — stored, fixed-Huffman and dynamic-Huffman blocks — so a file
//! any encoder wrote opens rather than only the ones `encode` writes. A 16-bit
//! sample is read at its high byte, because the canvas is eight bits a channel.
//!
//! **An interlaced PNG is refused and paints the placeholder.** Adam7 is seven
//! reduced images with their own filters, and no PNG a snapshot embeds is
//! written that way — an icon is not progressively loaded off a disk that is
//! not being read.
//!
//! # SVG
//!
//! Enough of it to draw an icon, through the painter's own rasterizer:
//!
//! ```text
//! svg width height viewBox   g   rect circle ellipse line polyline polygon
//! path                       M L H V C S Q T A Z, absolute and relative
//! fill stroke stroke-width   stroke-linecap stroke-linejoin
//! opacity fill-opacity stroke-opacity
//! transform                  translate scale rotate matrix
//! colours                    #rgb #rgba #rrggbb #rrggbbaa, rgb(), rgba(),
//!                            currentColor, none, and the basic HTML names
//! ```
//!
//! Outside that subset nothing is drawn and the rest of the picture still is:
//! `text`, `image`, `use`, gradients, patterns, filters, masks, clip paths,
//! markers, a `style` element or attribute, `fill-rule`, `skewX`/`skewY`, and
//! `preserveAspectRatio` — a picture fills the box the layout gave it, which is
//! the rule the raster half has always followed.
//!
//! `opacity` multiplies into the colours below it rather than compositing a
//! group, which is the simplification `paint.rs` already documents for the
//! element tree.
//!
//! # The size a picture takes
//!
//! What the DOM does, in one sentence each:
//!
//! * a box that declares no size takes the picture's own — a PNG's pixels, an
//!   SVG's `width` and `height`, or its `viewBox` where it has only that;
//! * a box that declares one scales the picture into it;
//! * an SVG is re-rasterized at the box's size rather than scaled, so an icon
//!   in a box four times its `viewBox` is four times as sharp.

use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

use super::{Box2, DISTANCES, LENGTHS, Rgba, mul255, over, paeth};

// ---------------------------------------------------------------------------
// The public surface
// ---------------------------------------------------------------------------

/// A source the painter could read.
pub(super) enum Picture {
    Raster(Image),
    Vector(Svg),
}

impl Picture {
    /// The size a page gives this picture when no rule sizes it.
    pub(super) fn intrinsic(&self) -> (f32, f32) {
        match self {
            Self::Raster(image) => (image.width as f32, image.height as f32),
            Self::Vector(svg) => svg.size,
        }
    }

    /// Draws it into the box the layout gave it.
    pub(super) fn draw(
        &self,
        canvas: &mut Pixmap,
        box_: Box2,
        opacity: f32,
        colour: Rgba,
        clip: Option<&Mask>,
    ) {
        match self {
            Self::Raster(image) => scaled(canvas, image, box_, opacity, clip),
            Self::Vector(svg) => svg.draw(canvas, box_, opacity, colour, clip),
        }
    }
}

/// The picture a `data:` URI holds, or `None` for a source this painter cannot
/// read.
pub(super) fn read(source: &str) -> Option<Picture> {
    let (kind, body) = data_uri(source)?;
    match kind.as_str() {
        "image/png" => decode(&body).ok().map(Picture::Raster),
        "image/svg+xml" => {
            Svg::parse(core::str::from_utf8(&body).ok()?).map(Picture::Vector)
        }
        _ => None,
    }
}

/// The artwork an `icon` carries, which is the SVG itself rather than a source
/// to fetch.
///
/// Nothing is decoded on the way in: the scene holds the drawing whole, which
/// is what lets `paint.rs` draw it with `currentColor` bound to the colour the
/// element paints in.
pub(super) fn artwork(source: &str) -> Option<Picture> {
    Svg::parse(source).map(Picture::Vector)
}

/// `data:<type>[;charset=…][;base64],<body>`, split into the type and the
/// bytes. A body that is not base64 is percent-encoded, which is the other
/// form a page writes an SVG in.
fn data_uri(source: &str) -> Option<(String, Vec<u8>)> {
    let rest = source.strip_prefix("data:")?;
    let (meta, body) = rest.split_once(',')?;
    let mut parts = meta.split(';');
    let kind = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let base64ed = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    let bytes = if base64ed { base64(body)? } else { percent(body) };
    Some((kind, bytes))
}

/// A base64 body, decoded. Whitespace is skipped, padding is optional, and any
/// other character answers `None`.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0_u32;
    for c in text.bytes() {
        let sextet = match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | sextet;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// `%xx` back to a byte. A `%` that is not two hex digits stands for itself,
/// and `+` is a plus: this is a URI and not a form field.
fn percent(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0_usize;
    while let Some(&byte) = bytes.get(at) {
        let pair = |i: usize| {
            let hi = char::from(*bytes.get(i)?).to_digit(16)?;
            let lo = char::from(*bytes.get(i.checked_add(1)?)?).to_digit(16)?;
            u8::try_from(hi.checked_mul(16)?.checked_add(lo)?).ok()
        };
        match (byte, pair(at.saturating_add(1))) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                at = at.saturating_add(3);
            }
            _ => {
                out.push(byte);
                at = at.saturating_add(1);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// A raster, and how it lands on the canvas
// ---------------------------------------------------------------------------

/// The pixels of a PNG, straight (un-premultiplied) 8-bit RGBA.
#[derive(Debug)]
pub(super) struct Image {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
}

impl Image {
    pub(super) fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?
            .checked_mul(4)?;
        let slice = self.rgba.get(i..i.checked_add(4)?)?;
        Some([*slice.first()?, *slice.get(1)?, *slice.get(2)?, *slice.get(3)?])
    }
}

pub(super) fn pixel_bytes(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| format!("the image {width}x{height} is too large to hold"))
}

/// Draws an image into a box, nearest neighbour.
///
/// The source pixel for a device pixel is picked in integers — `(x - left) *
/// width / box width` — so a scaled picture is the same picture on every host,
/// which is the rule the whole painter is written to.
fn scaled(canvas: &mut Pixmap, image: &Image, box_: Box2, opacity: f32, clip: Option<&Mask>) {
    let (width, height) = (box_.r - box_.l, box_.b - box_.t);
    if width <= 0 || height <= 0 || image.width == 0 || image.height == 0 {
        return;
    }
    let (cw, ch) = (canvas.width(), canvas.height());
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let pixels = canvas.pixels_mut();
    for y in box_.t.max(0)..box_.b.min(ch as i32) {
        let sy = (i64::from(y - box_.t) * i64::from(image.height)) / i64::from(height);
        for x in box_.l.max(0)..box_.r.min(cw as i32) {
            let sx = (i64::from(x - box_.l) * i64::from(image.width)) / i64::from(width);
            let (Ok(sx), Ok(sy)) = (u32::try_from(sx), u32::try_from(sy)) else { continue };
            let Some(source) = image.pixel(sx, sy) else { continue };
            let i = (y as u32 as usize).saturating_mul(cw as usize).saturating_add(x as usize);
            let coverage = clip.map_or(255, |mask| mask.data().get(i).copied().unwrap_or(0));
            let a = mul255(mul255(source[3], coverage), alpha);
            if a == 0 {
                continue;
            }
            let src = [mul255(source[0], a), mul255(source[1], a), mul255(source[2], a), a];
            if let Some(slot) = pixels.get_mut(i) {
                *slot = over(src, *slot);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// The largest picture this will decode along either axis. A snapshot is at
/// most a viewport, and an icon is two dozen pixels; the cap is here so a
/// header claiming four billion rows is a sentence rather than an allocation.
const MAX_SIDE: u32 = 8192;

/// A PNG's pixels, whatever colour type, bit depth and transparency it was
/// written with.
///
/// # Errors
/// One sentence naming what it could not read. An interlaced file is one of
/// them, deliberately — the header of this file says why.
pub(super) fn decode(png: &[u8]) -> Result<Image, String> {
    let bad = |what: &str| format!("the PNG {what}");
    if png.get(..8) != Some(&SIGNATURE) {
        return Err(bad("does not start with a PNG signature"));
    }
    let mut offset = 8_usize;
    let mut header: Option<Header> = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut transparency: Vec<u8> = Vec::new();
    let mut zlib: Vec<u8> = Vec::new();

    while offset < png.len() {
        let head = png.get(offset..offset.saturating_add(8)).ok_or_else(|| bad("ends mid-chunk"))?;
        let len = be32(head, 0).ok_or_else(|| bad("ends mid-chunk"))? as usize;
        let kind = head.get(4..8).ok_or_else(|| bad("ends mid-chunk"))?.to_vec();
        let start = offset.saturating_add(8);
        let end = start.checked_add(len).ok_or_else(|| bad("names a chunk longer than itself"))?;
        let data = png.get(start..end).ok_or_else(|| bad("names a chunk longer than itself"))?;
        match kind.as_slice() {
            b"IHDR" => header = Some(Header::parse(data)?),
            b"PLTE" => palette = data.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => transparency = data.to_vec(),
            b"IDAT" => zlib.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        offset = end.saturating_add(4);
    }
    let header = header.ok_or_else(|| bad("has no IHDR"))?;
    // Capped at what the header asks for, so a small chunk cannot inflate into
    // a gigabyte before the length check below has a chance to refuse it.
    let raw = inflate_capped(&zlib, header.raw_bytes()?)?;
    let rows = unfiltered(&header, &raw)?;
    header.expand(&rows, &palette, &transparency)
}

/// What IHDR says, checked.
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    colour: u8,
}

impl Header {
    fn parse(data: &[u8]) -> Result<Self, String> {
        let bad = |what: &str| format!("the PNG {what}");
        if data.len() != 13 {
            return Err(bad("has an IHDR that is not thirteen bytes"));
        }
        let width = be32(data, 0).ok_or_else(|| bad("has an unreadable IHDR"))?;
        let height = be32(data, 4).ok_or_else(|| bad("has an unreadable IHDR"))?;
        let depth = data.get(8).copied().unwrap_or(0);
        let colour = data.get(9).copied().unwrap_or(255);
        let interlace = data.get(12).copied().unwrap_or(0);
        if width == 0 || height == 0 {
            return Err(bad(&format!("is {width}x{height}, which has no pixels")));
        }
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(bad(&format!("is {width}x{height}, larger than this painter reads")));
        }
        if data.get(10) != Some(&0) || data.get(11) != Some(&0) {
            return Err(bad("uses a compression or filter method that does not exist"));
        }
        if interlace != 0 {
            return Err(bad("is interlaced, which this painter does not read"));
        }
        let ok = match colour {
            0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
            3 => matches!(depth, 1 | 2 | 4 | 8),
            2 | 4 | 6 => matches!(depth, 8 | 16),
            _ => false,
        };
        if !ok {
            return Err(bad(&format!("is colour type {colour} at {depth} bits, which is not a PNG")));
        }
        Ok(Self { width, height, depth, colour })
    }

    /// Samples to a pixel: grey, RGB, palette index, grey and alpha, RGBA.
    fn channels(&self) -> usize {
        match self.colour {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }

    /// A row's bytes, rounded up: a four-pixel row of one-bit grey is one byte.
    fn stride(&self) -> Option<usize> {
        let bits = (self.width as usize)
            .checked_mul(self.channels())?
            .checked_mul(usize::from(self.depth))?;
        Some(bits.checked_add(7)? / 8)
    }

    /// The whole raw stream: a filter byte and a row, per row.
    fn raw_bytes(&self) -> Result<usize, String> {
        let bad = || format!("the PNG {}x{} is too large to hold", self.width, self.height);
        self.stride()
            .and_then(|stride| stride.checked_add(1))
            .and_then(|row| row.checked_mul(self.height as usize))
            .ok_or_else(bad)
    }

    /// What the row filters step back by, which is a whole pixel or one byte,
    /// whichever is larger.
    fn filter_step(&self) -> usize {
        (self.channels().saturating_mul(usize::from(self.depth)) / 8).max(1)
    }

    /// The rows as straight RGBA, with the palette and `tRNS` applied.
    fn expand(
        &self,
        rows: &[Vec<u8>],
        palette: &[[u8; 3]],
        transparency: &[u8],
    ) -> Result<Image, String> {
        let bad = |what: &str| format!("the PNG {what}");
        if self.colour == 3 && palette.is_empty() {
            return Err(bad("is a palette image with no PLTE"));
        }
        let mut rgba = Vec::with_capacity(pixel_bytes(self.width, self.height)?);
        let clear = transparent(self.colour, transparency);
        for row in rows {
            let mut samples = Samples::new(row, self.depth);
            for _ in 0..self.width {
                let mut pixel = [0_u16; 4];
                for slot in pixel.iter_mut().take(self.channels()) {
                    *slot = samples.next().ok_or_else(|| bad("ends mid-row"))?;
                }
                let out = match self.colour {
                    0 => {
                        let grey = self.scale(pixel[0]);
                        [grey, grey, grey, 255]
                    }
                    2 => [
                        self.scale(pixel[0]),
                        self.scale(pixel[1]),
                        self.scale(pixel[2]),
                        255,
                    ],
                    3 => {
                        let at = usize::from(pixel[0]);
                        let entry =
                            palette.get(at).ok_or_else(|| bad("names a palette entry it has not"))?;
                        let alpha = transparency.get(at).copied().unwrap_or(255);
                        [
                            u16::from(entry[0]),
                            u16::from(entry[1]),
                            u16::from(entry[2]),
                            u16::from(alpha),
                        ]
                    }
                    4 => {
                        let grey = self.scale(pixel[0]);
                        [grey, grey, grey, self.scale(pixel[1])]
                    }
                    _ => [
                        self.scale(pixel[0]),
                        self.scale(pixel[1]),
                        self.scale(pixel[2]),
                        self.scale(pixel[3]),
                    ],
                };
                // `tRNS` on a grey or colour image names one exact sample
                // value, which is compared at the file's own depth rather than
                // at the canvas's eight bits.
                let hidden = match (clear, self.colour) {
                    (Some(key), 0) => pixel[0] == key[0],
                    (Some(key), 2) => [pixel[0], pixel[1], pixel[2]] == key,
                    _ => false,
                };
                rgba.extend_from_slice(&[
                    out[0] as u8,
                    out[1] as u8,
                    out[2] as u8,
                    if hidden { 0 } else { out[3] as u8 },
                ]);
            }
        }
        Ok(Image { width: self.width, height: self.height, rgba })
    }

    /// A sample at this file's depth, as an eight-bit one. One, two and four
    /// bits spread over the whole range; sixteen keeps its high byte.
    fn scale(&self, sample: u16) -> u16 {
        match self.depth {
            16 => sample >> 8,
            8 => sample,
            _ => {
                let max = (1_u32 << self.depth.min(15)).saturating_sub(1).max(1);
                (u32::from(sample).saturating_mul(255) / max) as u16
            }
        }
    }
}

/// The `tRNS` colour key, at the file's own depth, for the two colour types
/// that have one.
fn transparent(colour: u8, transparency: &[u8]) -> Option<[u16; 3]> {
    let at = |i: usize| {
        Some(u16::from_be_bytes([
            *transparency.get(i)?,
            *transparency.get(i.checked_add(1)?)?,
        ]))
    };
    match colour {
        0 if transparency.len() >= 2 => Some([at(0)?, 0, 0]),
        2 if transparency.len() >= 6 => Some([at(0)?, at(2)?, at(4)?]),
        _ => None,
    }
}

/// The samples of one row, whatever the bit depth packs them at.
struct Samples<'a> {
    row: &'a [u8],
    depth: u8,
    at: usize,
}

impl<'a> Samples<'a> {
    fn new(row: &'a [u8], depth: u8) -> Self {
        Self { row, depth, at: 0 }
    }

    fn next(&mut self) -> Option<u16> {
        let value = match self.depth {
            16 => {
                let i = self.at.checked_mul(2)?;
                u16::from_be_bytes([*self.row.get(i)?, *self.row.get(i.checked_add(1)?)?])
            }
            8 => u16::from(*self.row.get(self.at)?),
            _ => {
                let per = (8 / usize::from(self.depth)).max(1);
                let byte = u32::from(*self.row.get(self.at / per)?);
                let shift = (per - 1 - self.at % per).saturating_mul(usize::from(self.depth));
                let mask = (1_u32 << self.depth.min(15)).saturating_sub(1);
                u16::try_from((byte >> shift) & mask).ok()?
            }
        };
        self.at = self.at.checked_add(1)?;
        Some(value)
    }
}

/// The rows of the raw stream, each with its filter undone.
fn unfiltered(header: &Header, raw: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let bad = |what: &str| format!("the PNG {what}");
    let stride = header.stride().ok_or_else(|| bad("is too wide to hold"))?;
    let step = header.filter_step();
    if raw.len() != header.raw_bytes()? {
        return Err(bad("holds fewer rows than its header says"));
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(header.height as usize);
    let mut previous = vec![0_u8; stride];
    for index in 0..header.height as usize {
        let start = index.saturating_mul(stride.saturating_add(1));
        let kind = raw.get(start).copied().ok_or_else(|| bad("ends mid-row"))?;
        let from = start.saturating_add(1);
        let line = raw.get(from..from.saturating_add(stride)).ok_or_else(|| bad("ends mid-row"))?;
        let mut row = line.to_vec();
        unfilter(kind, &mut row, &previous, step)?;
        previous.clone_from(&row);
        out.push(row);
    }
    Ok(out)
}

/// One filtered row, put back. `row` is rebuilt left to right, so the byte the
/// filters call `a` is already unfiltered by the time it is read.
pub(super) fn unfilter(
    kind: u8,
    row: &mut [u8],
    previous: &[u8],
    step: usize,
) -> Result<(), String> {
    if kind > 4 {
        return Err(format!("the PNG uses row filter {kind}, which does not exist"));
    }
    for i in 0..row.len() {
        let left = |r: &[u8]| i.checked_sub(step).and_then(|j| r.get(j)).copied().unwrap_or(0);
        let a = left(row);
        let b = previous.get(i).copied().unwrap_or(0);
        let c = left(previous);
        let add = match kind {
            1 => a,
            2 => b,
            3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
            4 => paeth(a, b, c),
            _ => 0,
        };
        if let Some(slot) = row.get_mut(i) {
            *slot = slot.wrapping_add(add);
        }
    }
    Ok(())
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let slice = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([*slice.first()?, *slice.get(1)?, *slice.get(2)?, *slice.get(3)?]))
}

// ---------------------------------------------------------------------------
// Inflate
// ---------------------------------------------------------------------------

/// The other end of `paint.rs`'s `BitWriter`.
struct BitReader<'a> {
    data: &'a [u8],
    at: usize,
    bit: u32,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at)?;
        let value = (u32::from(byte) >> self.bit) & 1;
        self.bit = self.bit.saturating_add(1);
        if self.bit == 8 {
            self.bit = 0;
            self.at = self.at.saturating_add(1);
        }
        Some(value)
    }

    fn bits(&mut self, width: u32) -> Option<u32> {
        let mut value = 0;
        for i in 0..width {
            value |= self.bit()? << i;
        }
        Some(value)
    }

    /// To the next byte boundary, which is where a stored block's length sits.
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.at = self.at.saturating_add(1);
        }
    }
}

/// A canonical Huffman code, as its symbol counts per length and its symbols in
/// canonical order. Decoding walks a bit at a time, which is RFC 1951's own
/// description of the code and needs no table.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Option<Self> {
        let mut counts = [0_u16; 16];
        for &length in lengths {
            let slot = counts.get_mut(usize::from(length))?;
            *slot = slot.checked_add(1)?;
        }
        if counts.first().copied().unwrap_or(0) as usize == lengths.len() {
            return None;
        }
        let mut offsets = [0_u16; 16];
        let mut total = 0_u16;
        for length in 1..16_usize {
            *offsets.get_mut(length)? = total;
            total = total.checked_add(counts.get(length).copied().unwrap_or(0))?;
        }
        let mut symbols = vec![0_u16; usize::from(total)];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let slot = offsets.get_mut(usize::from(length))?;
            *symbols.get_mut(usize::from(*slot))? = u16::try_from(symbol).ok()?;
            *slot = slot.checked_add(1)?;
        }
        Some(Self { counts, symbols })
    }

    fn decode(&self, reader: &mut BitReader) -> Option<u16> {
        let mut code = 0_i32;
        let mut first = 0_i32;
        let mut index = 0_i32;
        for length in 1..16_usize {
            code |= i32::try_from(reader.bit()?).ok()?;
            let count = i32::from(self.counts.get(length).copied().unwrap_or(0));
            if code.checked_sub(first)? < count {
                let at = index.checked_add(code.checked_sub(first)?)?;
                return self.symbols.get(usize::try_from(at).ok()?).copied();
            }
            index = index.checked_add(count)?;
            first = first.checked_add(count)?.checked_shl(1)?;
            code = code.checked_shl(1)?;
        }
        None
    }
}

/// The fixed literal/length and distance trees, RFC 1951 §3.2.6.
fn fixed_trees() -> Option<(Huffman, Huffman)> {
    let mut lengths = [8_u8; 288];
    for (symbol, slot) in lengths.iter_mut().enumerate() {
        *slot = match symbol {
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    Some((Huffman::new(&lengths)?, Huffman::new(&[5_u8; 30])?))
}

/// A zlib stream, unwrapped. All of RFC 1951: stored, fixed-Huffman and
/// dynamic-Huffman blocks, in any order and any number.
pub(super) fn inflate(zlib: &[u8]) -> Result<Vec<u8>, String> {
    inflate_capped(zlib, usize::MAX)
}

/// The same, refusing anything that writes more than `cap` bytes.
fn inflate_capped(zlib: &[u8], cap: usize) -> Result<Vec<u8>, String> {
    let bad = |what: &str| format!("the PNG's compressed data {what}");
    let (Some(&cmf), Some(&flg)) = (zlib.first(), zlib.get(1)) else {
        return Err(bad("is shorter than a zlib header"));
    };
    if cmf & 0x0f != 8 {
        return Err(bad("is not deflate"));
    }
    if flg & 0x20 != 0 {
        return Err(bad("wants a preset dictionary"));
    }
    let mut reader = BitReader { data: zlib, at: 2, bit: 0 };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = reader.bits(1).ok_or_else(|| bad("ends mid-block"))?;
        match reader.bits(2).ok_or_else(|| bad("ends mid-block"))? {
            0 => {
                reader.align();
                let len = reader.bits(16).ok_or_else(|| bad("ends mid-block"))? as usize;
                reader.bits(16).ok_or_else(|| bad("ends mid-block"))?;
                if out.len().saturating_add(len) > cap {
                    return Err(bad("writes more than the header says the image holds"));
                }
                for _ in 0..len {
                    let byte = reader.bits(8).ok_or_else(|| bad("ends mid-block"))?;
                    out.push(byte as u8);
                }
            }
            1 => {
                let (literals, distances) =
                    fixed_trees().ok_or_else(|| bad("has no fixed tree, which cannot happen"))?;
                block(&mut reader, &mut out, &literals, &distances, cap)?;
            }
            2 => {
                let (literals, distances) = dynamic_trees(&mut reader)?;
                block(&mut reader, &mut out, &literals, &distances, cap)?;
            }
            _ => return Err(bad("uses block type 3, which does not exist")),
        }
        if out.len() > cap {
            return Err(bad("writes more than the header says the image holds"));
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

/// The order the code-length code's own lengths arrive in, RFC 1951 §3.2.7.
const CODE_LENGTH_ORDER: [usize; 19] =
    [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// A dynamic block's two trees, read out of the code-length code ahead of them.
fn dynamic_trees(reader: &mut BitReader) -> Result<(Huffman, Huffman), String> {
    let bad = |what: &str| format!("the PNG's compressed data {what}");
    let literals = reader.bits(5).ok_or_else(|| bad("ends mid-header"))? as usize + 257;
    let distances = reader.bits(5).ok_or_else(|| bad("ends mid-header"))? as usize + 1;
    let counts = reader.bits(4).ok_or_else(|| bad("ends mid-header"))? as usize + 4;
    if literals > 288 || distances > 32 {
        return Err(bad("declares more codes than the format has"));
    }

    let mut code_lengths = [0_u8; 19];
    for &slot in CODE_LENGTH_ORDER.iter().take(counts) {
        let value = reader.bits(3).ok_or_else(|| bad("ends mid-header"))?;
        if let Some(cell) = code_lengths.get_mut(slot) {
            *cell = value as u8;
        }
    }
    let code = Huffman::new(&code_lengths).ok_or_else(|| bad("has an empty code-length tree"))?;

    let total = literals.saturating_add(distances);
    let mut lengths: Vec<u8> = Vec::with_capacity(total);
    while lengths.len() < total {
        let symbol = code.decode(reader).ok_or_else(|| bad("holds a code no tree has"))?;
        match symbol {
            0..=15 => lengths.push(symbol as u8),
            16 => {
                let last =
                    lengths.last().copied().ok_or_else(|| bad("repeats a length before one"))?;
                let more = reader.bits(2).ok_or_else(|| bad("ends mid-header"))? + 3;
                for _ in 0..more {
                    lengths.push(last);
                }
            }
            17 => {
                let more = reader.bits(3).ok_or_else(|| bad("ends mid-header"))? + 3;
                for _ in 0..more {
                    lengths.push(0);
                }
            }
            18 => {
                let more = reader.bits(7).ok_or_else(|| bad("ends mid-header"))? + 11;
                for _ in 0..more {
                    lengths.push(0);
                }
            }
            _ => return Err(bad("names a code-length symbol that does not exist")),
        }
    }
    if lengths.len() != total {
        return Err(bad("declares more code lengths than it has codes"));
    }
    let (lit, dist) = lengths.split_at(literals);
    let literals = Huffman::new(lit).ok_or_else(|| bad("has an empty literal tree"))?;
    // Every distance length zero is legal: a block of literals alone.
    let distances = Huffman::new(dist).unwrap_or(Huffman { counts: [0; 16], symbols: Vec::new() });
    Ok((literals, distances))
}

/// One Huffman-coded block, whichever pair of trees it was coded with.
fn block(
    reader: &mut BitReader,
    out: &mut Vec<u8>,
    literals: &Huffman,
    distances: &Huffman,
    cap: usize,
) -> Result<(), String> {
    let bad = |what: &str| format!("the PNG's compressed data {what}");
    loop {
        if out.len() > cap {
            return Err(bad("writes more than the header says the image holds"));
        }
        let symbol = literals.decode(reader).ok_or_else(|| bad("holds a code no tree has"))?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = usize::from(symbol).saturating_sub(257);
                let (base, extra) =
                    LENGTHS.get(index).copied().ok_or_else(|| bad("names no length"))?;
                let more = reader.bits(u32::from(extra)).ok_or_else(|| bad("ends mid-match"))?;
                let length = usize::from(base).saturating_add(more as usize);

                let which = usize::from(
                    distances.decode(reader).ok_or_else(|| bad("holds a code no tree has"))?,
                );
                let (first, dextra) =
                    DISTANCES.get(which).copied().ok_or_else(|| bad("names no distance"))?;
                let more = reader.bits(u32::from(dextra)).ok_or_else(|| bad("ends mid-match"))?;
                let distance = usize::from(first).saturating_add(more as usize);

                if distance == 0 || distance > out.len() {
                    return Err(bad("copies from before the start of the image"));
                }
                let from = out.len().saturating_sub(distance);
                for step in 0..length {
                    let byte = out
                        .get(from.saturating_add(step))
                        .copied()
                        .ok_or_else(|| bad("copies past what it has written"))?;
                    out.push(byte);
                }
            }
            _ => return Err(bad("names a symbol the tree does not have")),
        }
    }
}

// ---------------------------------------------------------------------------
// SVG
// ---------------------------------------------------------------------------

/// An SVG, flattened to the shapes the painter can draw.
pub(super) struct Svg {
    /// The user-space rectangle the shapes live in: the `viewBox`, or the
    /// `width` and `height` where there is none.
    view: [f32; 4],
    /// What a page gives it when nothing sizes it.
    size: (f32, f32),
    shapes: Vec<Shape>,
}

/// One drawn element, in user space, with the transform its ancestors gave it.
struct Shape {
    path: Path,
    transform: Transform,
    fill: Option<Ink>,
    stroke: Option<(Ink, Pen)>,
}

/// A colour and the alpha the tree multiplied onto it. `None` for the colour is
/// `currentColor`, which the painter resolves against the element's own `color`.
#[derive(Clone, Copy)]
struct Ink {
    colour: Option<[u8; 3]>,
    alpha: f32,
}

#[derive(Clone, Copy)]
struct Pen {
    width: f32,
    cap: LineCap,
    join: LineJoin,
}

/// What an element inherits from the element above it.
#[derive(Clone)]
struct Inherited {
    fill: Option<Option<[u8; 3]>>,
    stroke: Option<Option<[u8; 3]>>,
    width: f32,
    cap: LineCap,
    join: LineJoin,
    opacity: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
    transform: Transform,
}

impl Default for Inherited {
    fn default() -> Self {
        Self {
            // A page paints an unfilled shape black, which is what an icon with
            // no `fill` anywhere depends on.
            fill: Some(Some([0, 0, 0])),
            stroke: None,
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            opacity: 1.0,
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            transform: Transform::identity(),
        }
    }
}

/// Elements whose children are not pictures: definitions, text, and the things
/// that describe a paint rather than apply one.
const NOT_DRAWN: [&str; 14] = [
    "defs",
    "text",
    "style",
    "title",
    "desc",
    "metadata",
    "linearGradient",
    "radialGradient",
    "pattern",
    "filter",
    "mask",
    "clipPath",
    "marker",
    "symbol",
];

impl Svg {
    /// The shapes of an SVG document, or `None` for one with no coordinate
    /// system to draw them in.
    fn parse(text: &str) -> Option<Self> {
        let mut tags = Tags::new(text);
        let mut stack: Vec<Inherited> = vec![Inherited::default()];
        let mut shapes: Vec<Shape> = Vec::new();
        let mut root: Option<([f32; 4], (f32, f32))> = None;
        let mut depth = 0_usize;
        let mut skip_from: Option<usize> = None;

        while let Some(tag) = tags.next() {
            match tag {
                Tag::Open { name, attributes, empty } => {
                    let inherited = match skip_from {
                        Some(_) => stack.last()?.clone(),
                        None => {
                            let inherited = stack.last()?.apply(&attributes);
                            if name == "svg" && root.is_none() {
                                root = frame(&attributes);
                            } else {
                                shape(&name, &attributes, &inherited, &mut shapes);
                            }
                            if NOT_DRAWN.contains(&name.as_str()) && !empty {
                                skip_from = Some(depth);
                            }
                            inherited
                        }
                    };
                    if !empty {
                        stack.push(inherited);
                        depth = depth.saturating_add(1);
                    }
                }
                Tag::Close { .. } => {
                    depth = depth.saturating_sub(1);
                    if stack.len() > 1 {
                        stack.pop();
                    }
                    if skip_from == Some(depth) {
                        skip_from = None;
                    }
                }
            }
        }
        let (view, size) = root?;
        Some(Self { view, size, shapes })
    }

    /// Draws the shapes into the box, with the box's own pixels as the unit.
    ///
    /// The rasterizer is the painter's, so an icon is anti-aliased the way a
    /// border is, and it is re-rasterized at whatever size the box is rather
    /// than scaled from a smaller one.
    fn draw(
        &self,
        canvas: &mut Pixmap,
        box_: Box2,
        opacity: f32,
        colour: Rgba,
        clip: Option<&Mask>,
    ) {
        let (width, height) = ((box_.r - box_.l) as f32, (box_.b - box_.t) as f32);
        if width <= 0.0 || height <= 0.0 || self.view[2] <= 0.0 || self.view[3] <= 0.0 {
            return;
        }
        let outer = Transform::from_translate(box_.l as f32, box_.t as f32)
            .pre_concat(Transform::from_scale(width / self.view[2], height / self.view[3]))
            .pre_concat(Transform::from_translate(-self.view[0], -self.view[1]));

        for shape in &self.shapes {
            let transform = outer.pre_concat(shape.transform);
            if let Some(ink) = shape.fill {
                let paint = Paint {
                    anti_alias: true,
                    shader: ink.shade(colour, opacity),
                    ..Paint::default()
                };
                canvas.fill_path(&shape.path, &paint, FillRule::Winding, transform, clip);
            }
            if let Some((ink, pen)) = shape.stroke {
                if pen.width <= 0.0 {
                    continue;
                }
                let paint = Paint {
                    anti_alias: true,
                    shader: ink.shade(colour, opacity),
                    ..Paint::default()
                };
                let stroke = Stroke {
                    width: pen.width,
                    line_cap: pen.cap,
                    line_join: pen.join,
                    ..Stroke::default()
                };
                canvas.stroke_path(&shape.path, &paint, &stroke, transform, clip);
            }
        }
    }
}

impl Ink {
    fn shade<'a>(self, current: Rgba, opacity: f32) -> tiny_skia::Shader<'a> {
        let [r, g, b] = self.colour.unwrap_or([current.r, current.g, current.b]);
        let a = (self.alpha * opacity * if self.colour.is_none() { current.a } else { 1.0 })
            .clamp(0.0, 1.0);
        let solid =
            tiny_skia::Color::from_rgba(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, a)
                .unwrap_or(tiny_skia::Color::TRANSPARENT);
        tiny_skia::Shader::SolidColor(solid)
    }
}

/// The root element's coordinate system and the size a page gives it.
fn frame(attributes: &[(String, String)]) -> Option<([f32; 4], (f32, f32))> {
    let declared = |name: &str| attribute(attributes, name).and_then(|v| length(&v));
    let (width, height) = (declared("width"), declared("height"));
    let view = attribute(attributes, "viewBox").and_then(|v| {
        let mut numbers = v.split([',', ' ', '\t', '\n', '\r']).filter(|p| !p.is_empty());
        let mut next = || numbers.next().and_then(|n| n.parse::<f32>().ok());
        Some([next()?, next()?, next()?, next()?])
    });
    match (view, width, height) {
        (Some(view), Some(w), Some(h)) => Some((view, (w, h))),
        (Some(view), _, _) => Some((view, (view[2], view[3]))),
        (None, Some(w), Some(h)) => Some(([0.0, 0.0, w, h], (w, h))),
        _ => None,
    }
}

impl Inherited {
    /// This element's own presentation attributes, over what it inherited.
    fn apply(&self, attributes: &[(String, String)]) -> Self {
        let mut out = self.clone();
        let number = |name: &str| attribute(attributes, name).and_then(|v| length(&v));
        if let Some(value) = attribute(attributes, "fill") {
            out.fill = ink(&value);
        }
        if let Some(value) = attribute(attributes, "stroke") {
            out.stroke = ink(&value);
        }
        if let Some(width) = number("stroke-width") {
            out.width = width;
        }
        if let Some(value) = attribute(attributes, "stroke-linecap") {
            out.cap = match value.as_str() {
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                _ => LineCap::Butt,
            };
        }
        if let Some(value) = attribute(attributes, "stroke-linejoin") {
            out.join = match value.as_str() {
                "round" => LineJoin::Round,
                "bevel" => LineJoin::Bevel,
                _ => LineJoin::Miter,
            };
        }
        if let Some(value) = number("opacity") {
            out.opacity *= value.clamp(0.0, 1.0);
        }
        if let Some(value) = number("fill-opacity") {
            out.fill_opacity = value.clamp(0.0, 1.0);
        }
        if let Some(value) = number("stroke-opacity") {
            out.stroke_opacity = value.clamp(0.0, 1.0);
        }
        if let Some(value) = attribute(attributes, "transform") {
            out.transform = out.transform.pre_concat(transform(&value));
        }
        out
    }

    fn fill_ink(&self) -> Option<Ink> {
        self.fill.map(|colour| Ink { colour, alpha: self.opacity * self.fill_opacity })
    }

    fn stroke_ink(&self) -> Option<(Ink, Pen)> {
        let colour = self.stroke?;
        Some((
            Ink { colour, alpha: self.opacity * self.stroke_opacity },
            Pen { width: self.width, cap: self.cap, join: self.join },
        ))
    }
}

/// One shape element, appended if this is one.
fn shape(
    name: &str,
    attributes: &[(String, String)],
    inherited: &Inherited,
    out: &mut Vec<Shape>,
) {
    let number = |key: &str| attribute(attributes, key).and_then(|v| length(&v)).unwrap_or(0.0);
    let mut builder = PathBuilder::new();
    match name {
        "rect" => {
            let (w, h) = (number("width"), number("height"));
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let (x, y) = (number("x"), number("y"));
            let rx = attribute(attributes, "rx").and_then(|v| length(&v));
            let ry = attribute(attributes, "ry").and_then(|v| length(&v));
            match (rx.or(ry), ry.or(rx)) {
                (Some(rx), Some(ry)) if rx > 0.0 && ry > 0.0 => {
                    rounded(&mut builder, x, y, w, h, rx.min(w / 2.0), ry.min(h / 2.0));
                }
                _ => {
                    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
                        builder.push_rect(rect);
                    }
                }
            }
        }
        "circle" | "ellipse" => {
            let (rx, ry) = if name == "circle" {
                (number("r"), number("r"))
            } else {
                (number("rx"), number("ry"))
            };
            if rx <= 0.0 || ry <= 0.0 {
                return;
            }
            let (cx, cy) = (number("cx"), number("cy"));
            if let Some(rect) = Rect::from_ltrb(cx - rx, cy - ry, cx + rx, cy + ry) {
                builder.push_oval(rect);
            }
        }
        "line" => {
            builder.move_to(number("x1"), number("y1"));
            builder.line_to(number("x2"), number("y2"));
        }
        "polyline" | "polygon" => {
            let points = attribute(attributes, "points").unwrap_or_default();
            let mut numbers = points
                .split([',', ' ', '\t', '\n', '\r'])
                .filter(|p| !p.is_empty())
                .filter_map(|n| n.parse::<f32>().ok());
            let mut first = true;
            while let (Some(x), Some(y)) = (numbers.next(), numbers.next()) {
                if first {
                    builder.move_to(x, y);
                    first = false;
                } else {
                    builder.line_to(x, y);
                }
            }
            if name == "polygon" && !first {
                builder.close();
            }
        }
        "path" => {
            let Some(data) = attribute(attributes, "d") else { return };
            draw_path(&mut builder, &data);
        }
        _ => return,
    }
    let Some(path) = builder.finish() else { return };
    let (fill, stroke) = (inherited.fill_ink(), inherited.stroke_ink());
    if fill.is_none() && stroke.is_none() {
        return;
    }
    out.push(Shape { path, transform: inherited.transform, fill, stroke });
}

/// A rounded rectangle, its corners as the four-cubic circle every rasterizer
/// draws one with.
fn rounded(builder: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) {
    // The distance along the tangent that makes a cubic a quarter ellipse.
    const K: f32 = 0.552_284_75;
    let (kx, ky) = (rx * K, ry * K);
    let (r, b) = (x + w, y + h);
    builder.move_to(x + rx, y);
    builder.line_to(r - rx, y);
    builder.cubic_to(r - rx + kx, y, r, y + ry - ky, r, y + ry);
    builder.line_to(r, b - ry);
    builder.cubic_to(r, b - ry + ky, r - rx + kx, b, r - rx, b);
    builder.line_to(x + rx, b);
    builder.cubic_to(x + rx - kx, b, x, b - ry + ky, x, b - ry);
    builder.line_to(x, y + ry);
    builder.cubic_to(x, y + ry - ky, x + rx - kx, y, x + rx, y);
    builder.close();
}

fn attribute(attributes: &[(String, String)], name: &str) -> Option<String> {
    attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.trim().to_string())
}

/// A user-space length: a number, with `px` allowed on it because a page writes
/// one. A percentage has nothing to be a percentage of here.
fn length(value: &str) -> Option<f32> {
    let text = value.trim();
    let text = text.strip_suffix("px").unwrap_or(text);
    let parsed = text.trim().parse::<f32>().ok()?;
    parsed.is_finite().then_some(parsed)
}

// ---------------------------------------------------------------------------
// Colours
// ---------------------------------------------------------------------------

/// The basic HTML colour names, which is what a hand-written icon uses when it
/// does not write a hex code.
const NAMED: [(&str, [u8; 3]); 17] = [
    ("black", [0, 0, 0]),
    ("silver", [192, 192, 192]),
    ("gray", [128, 128, 128]),
    ("grey", [128, 128, 128]),
    ("white", [255, 255, 255]),
    ("maroon", [128, 0, 0]),
    ("red", [255, 0, 0]),
    ("purple", [128, 0, 128]),
    ("fuchsia", [255, 0, 255]),
    ("green", [0, 128, 0]),
    ("lime", [0, 255, 0]),
    ("olive", [128, 128, 0]),
    ("yellow", [255, 255, 0]),
    ("navy", [0, 0, 128]),
    ("blue", [0, 0, 255]),
    ("teal", [0, 128, 128]),
    ("aqua", [0, 255, 255]),
];

/// `Some(None)` is `currentColor`; `None` is `none`, which paints nothing.
fn ink(value: &str) -> Option<Option<[u8; 3]>> {
    let text = value.trim();
    if text.eq_ignore_ascii_case("none") || text.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if text.eq_ignore_ascii_case("currentcolor") {
        return Some(None);
    }
    if let Some(hex) = text.strip_prefix('#') {
        let digit = |c: u8| char::from(c).to_digit(16).map(|d| d as u8);
        let bytes = hex.as_bytes();
        let short = |i: usize| digit(*bytes.get(i)?).map(|d| d * 17);
        let long = |i: usize| Some(digit(*bytes.get(i)?)? * 16 + digit(*bytes.get(i + 1)?)?);
        return match bytes.len() {
            3 | 4 => Some(Some([short(0)?, short(1)?, short(2)?])),
            6 | 8 => Some(Some([long(0)?, long(2)?, long(4)?])),
            _ => None,
        };
    }
    let lowered = text.to_ascii_lowercase();
    if let Some(inner) =
        lowered.strip_prefix("rgba(").or_else(|| lowered.strip_prefix("rgb(")).map(str::trim)
    {
        let inner = inner.strip_suffix(')')?;
        let mut parts = inner.split([',', ' ', '/']).filter(|p| !p.is_empty());
        let mut channel = || {
            let part = parts.next()?.trim();
            match part.strip_suffix('%') {
                Some(percent) => {
                    let value = percent.parse::<f32>().ok()?;
                    Some((value * 255.0 / 100.0).clamp(0.0, 255.0) as u8)
                }
                None => Some(part.parse::<f32>().ok()?.clamp(0.0, 255.0) as u8),
            }
        };
        return Some(Some([channel()?, channel()?, channel()?]));
    }
    NAMED.iter().find(|(name, _)| *name == lowered).map(|(_, rgb)| Some(*rgb))
}

// ---------------------------------------------------------------------------
// Transforms
// ---------------------------------------------------------------------------

/// `translate`, `scale`, `rotate` and `matrix`, applied left to right. Anything
/// else in the list is skipped, and what it was going to move stays put.
fn transform(value: &str) -> Transform {
    let mut out = Transform::identity();
    let mut rest = value;
    while let Some(open) = rest.find('(') {
        let (name, from) = rest.split_at(open);
        let Some((inside, after)) = from.get(1..).and_then(|f| f.split_once(')')) else {
            return out;
        };
        rest = after;
        let numbers: Vec<f32> = inside
            .split([',', ' ', '\t', '\n', '\r'])
            .filter(|p| !p.is_empty())
            .filter_map(|n| n.parse::<f32>().ok())
            .collect();
        let at = |i: usize| numbers.get(i).copied();
        let step = match (name.trim(), numbers.len()) {
            ("translate", 1..) => Transform::from_translate(at(0).unwrap_or(0.0), at(1).unwrap_or(0.0)),
            ("scale", 1) => Transform::from_scale(at(0).unwrap_or(1.0), at(0).unwrap_or(1.0)),
            ("scale", 2..) => Transform::from_scale(at(0).unwrap_or(1.0), at(1).unwrap_or(1.0)),
            ("rotate", 1) => Transform::from_rotate(at(0).unwrap_or(0.0)),
            ("rotate", 3..) => Transform::from_rotate_at(
                at(0).unwrap_or(0.0),
                at(1).unwrap_or(0.0),
                at(2).unwrap_or(0.0),
            ),
            ("matrix", 6..) => Transform::from_row(
                at(0).unwrap_or(1.0),
                at(1).unwrap_or(0.0),
                at(2).unwrap_or(0.0),
                at(3).unwrap_or(1.0),
                at(4).unwrap_or(0.0),
                at(5).unwrap_or(0.0),
            ),
            _ => continue,
        };
        out = out.pre_concat(step);
    }
    out
}

// ---------------------------------------------------------------------------
// Path data
// ---------------------------------------------------------------------------

/// `d`, command by command. A command the subset has no arm for stops the walk,
/// so what was drawn before it still paints.
fn draw_path(builder: &mut PathBuilder, data: &str) {
    let mut numbers = Numbers::new(data);
    let mut at = (0.0_f32, 0.0_f32);
    let mut start = at;
    let mut control = at;
    let mut command = b' ';
    let mut open = false;

    loop {
        let next = numbers.command();
        match next {
            Some(letter) => command = letter,
            // A repeated coordinate list means the previous command again, and
            // a repeated `M` means `L`.
            None if numbers.done() => break,
            None => {
                command = match command {
                    b'M' => b'L',
                    b'm' => b'l',
                    other => other,
                };
            }
        }
        // Every coordinate of a relative command is against the current point,
        // which is where it was when the command started.
        let base = if command.is_ascii_lowercase() { at } else { (0.0, 0.0) };
        let letter = command.to_ascii_uppercase();
        if letter != b'M' && letter != b'Z' && !open {
            builder.move_to(at.0, at.1);
            open = true;
        }
        match letter {
            b'M' => {
                let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                let to = (base.0 + x, base.1 + y);
                builder.move_to(to.0, to.1);
                (at, start, control, open) = (to, to, to, true);
            }
            b'L' => {
                let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                let to = (base.0 + x, base.1 + y);
                builder.line_to(to.0, to.1);
                (at, control) = (to, to);
            }
            b'H' | b'V' => {
                let Some(value) = numbers.number() else { break };
                let to = if letter == b'H' {
                    (base.0 + value, at.1)
                } else {
                    (at.0, base.1 + value)
                };
                builder.line_to(to.0, to.1);
                (at, control) = (to, to);
            }
            b'C' | b'S' => {
                let first = if letter == b'C' {
                    let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                    (base.0 + x, base.1 + y)
                } else {
                    // `S` reflects the last cubic's second handle.
                    (2.0 * at.0 - control.0, 2.0 * at.1 - control.1)
                };
                let (Some(x2), Some(y2)) = (numbers.number(), numbers.number()) else { break };
                let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                let second = (base.0 + x2, base.1 + y2);
                let to = (base.0 + x, base.1 + y);
                builder.cubic_to(first.0, first.1, second.0, second.1, to.0, to.1);
                (at, control) = (to, second);
            }
            b'Q' | b'T' => {
                let handle = if letter == b'Q' {
                    let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                    (base.0 + x, base.1 + y)
                } else {
                    (2.0 * at.0 - control.0, 2.0 * at.1 - control.1)
                };
                let (Some(x), Some(y)) = (numbers.number(), numbers.number()) else { break };
                let to = (base.0 + x, base.1 + y);
                builder.quad_to(handle.0, handle.1, to.0, to.1);
                (at, control) = (to, handle);
            }
            b'A' => {
                let (Some(rx), Some(ry)) = (numbers.number(), numbers.number()) else { break };
                let (Some(angle), Some(large)) = (numbers.number(), numbers.number()) else {
                    break;
                };
                let (Some(sweep), Some(x)) = (numbers.number(), numbers.number()) else { break };
                let Some(y) = numbers.number() else { break };
                let to = (base.0 + x, base.1 + y);
                arc(builder, at, (rx, ry), angle, large != 0.0, sweep != 0.0, to);
                (at, control) = (to, to);
            }
            b'Z' => {
                if open {
                    builder.close();
                }
                (at, control, open) = (start, start, false);
            }
            _ => break,
        }
    }
}

/// An elliptical arc, as the cubics every rasterizer draws one with: the
/// endpoint form of F.6.5 turned into a centre and a sweep, then split into
/// pieces of at most ninety degrees.
fn arc(
    builder: &mut PathBuilder,
    from: (f32, f32),
    radii: (f32, f32),
    degrees: f32,
    large: bool,
    sweep: bool,
    to: (f32, f32),
) {
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());
    if rx == 0.0 || ry == 0.0 || (from.0 == to.0 && from.1 == to.1) {
        builder.line_to(to.0, to.1);
        return;
    }
    let phi = degrees.to_radians();
    let (sin, cos) = phi.sin_cos();
    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;

    // F.6.6: radii too small for the two endpoints are scaled up until they fit.
    let over = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if over > 1.0 {
        let grow = over.sqrt();
        rx *= grow;
        ry *= grow;
    }
    let numerator = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    if denominator == 0.0 {
        builder.line_to(to.0, to.1);
        return;
    }
    let mut factor = (numerator / denominator).sqrt();
    if large == sweep {
        factor = -factor;
    }
    let cx1 = factor * rx * y1 / ry;
    let cy1 = -factor * ry * x1 / rx;
    let cx = cos * cx1 - sin * cy1 + (from.0 + to.0) / 2.0;
    let cy = sin * cx1 + cos * cy1 + (from.1 + to.1) / 2.0;

    let angle = |ux: f32, uy: f32| uy.atan2(ux);
    let start = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
    let end = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let mut delta = end - start;
    let turn = core::f32::consts::TAU;
    if !sweep && delta > 0.0 {
        delta -= turn;
    } else if sweep && delta < 0.0 {
        delta += turn;
    }

    let pieces = (delta.abs() / core::f32::consts::FRAC_PI_2).ceil().max(1.0) as i32;
    let step = delta / pieces as f32;
    // The tangent length that makes a cubic match a circular arc of `step`.
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let map = |a: f32| {
        let (s, c) = a.sin_cos();
        (cx + cos * rx * c - sin * ry * s, cy + sin * rx * c + cos * ry * s)
    };
    let tangent = |a: f32| {
        let (s, c) = a.sin_cos();
        (-cos * rx * s - sin * ry * c, -sin * rx * s + cos * ry * c)
    };
    for piece in 0..pieces {
        let a = start + step * piece as f32;
        let b = a + step;
        let (px, py) = map(a);
        let (qx, qy) = map(b);
        let (tax, tay) = tangent(a);
        let (tbx, tby) = tangent(b);
        builder.cubic_to(px + k * tax, py + k * tay, qx - k * tbx, qy - k * tby, qx, qy);
    }
}

/// The numbers and command letters of a `d` attribute, in order.
struct Numbers<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Numbers<'a> {
    fn new(text: &'a str) -> Self {
        Self { bytes: text.as_bytes(), at: 0 }
    }

    fn skip(&mut self) {
        while let Some(&byte) = self.bytes.get(self.at) {
            if byte == b',' || byte.is_ascii_whitespace() {
                self.at = self.at.saturating_add(1);
            } else {
                break;
            }
        }
    }

    fn done(&mut self) -> bool {
        self.skip();
        self.at >= self.bytes.len()
    }

    /// The next command letter, or `None` where a number comes next.
    fn command(&mut self) -> Option<u8> {
        self.skip();
        let byte = *self.bytes.get(self.at)?;
        if byte.is_ascii_alphabetic() {
            self.at = self.at.saturating_add(1);
            return Some(byte);
        }
        None
    }

    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.at;
        let mut seen_digit = false;
        let mut seen_dot = false;
        while let Some(&byte) = self.bytes.get(self.at) {
            match byte {
                b'+' | b'-' if self.at == start => {}
                b'+' | b'-'
                    if matches!(
                        self.bytes.get(self.at.saturating_sub(1)),
                        Some(b'e') | Some(b'E')
                    ) => {}
                b'0'..=b'9' => seen_digit = true,
                b'.' if !seen_dot => seen_dot = true,
                b'e' | b'E' if seen_digit => {}
                _ => break,
            }
            self.at = self.at.saturating_add(1);
        }
        let text = self.bytes.get(start..self.at).and_then(|b| core::str::from_utf8(b).ok())?;
        text.parse::<f32>().ok().filter(|n| n.is_finite())
    }
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// One element boundary. Text between tags is not a picture, so it is skipped.
enum Tag {
    Open { name: String, attributes: Vec<(String, String)>, empty: bool },
    Close { name: String },
}

/// The tags of a document, in order. Comments, processing instructions,
/// doctypes and CDATA are skipped whole.
struct Tags<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Tags<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, at: 0 }
    }

    fn next(&mut self) -> Option<Tag> {
        loop {
            let open = self.text.get(self.at..)?.find('<')?;
            let after = self.at.saturating_add(open).saturating_add(1);
            let body = self.text.get(after..)?;
            let skipped = [("!--", "-->"), ("![CDATA[", "]]>"), ("?", "?>"), ("!", ">")]
                .into_iter()
                .find(|(opener, _)| body.starts_with(opener));
            if let Some((opener, closer)) = skipped {
                let from = after.saturating_add(opener.len());
                self.at = self
                    .text
                    .get(from..)
                    .and_then(|tail| tail.find(closer))
                    .map_or(self.text.len(), |i| {
                        from.saturating_add(i).saturating_add(closer.len())
                    });
                continue;
            }
            let end = body.find('>')?;
            let inside = body.get(..end)?;
            self.at = after.saturating_add(end).saturating_add(1);
            if let Some(name) = inside.strip_prefix('/') {
                return Some(Tag::Close { name: name.trim().to_string() });
            }
            let empty = inside.ends_with('/');
            let inside = inside.strip_suffix('/').unwrap_or(inside);
            let split = inside
                .char_indices()
                .find(|(_, c)| c.is_whitespace())
                .map_or(inside.len(), |(i, _)| i);
            let name = inside.get(..split)?.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let attributes = parse_attributes(inside.get(split..).unwrap_or(""));
            return Some(Tag::Open { name, attributes, empty });
        }
    }
}

/// `name="value"` pairs, single or double quoted.
fn parse_attributes(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0_usize;
    while at < bytes.len() {
        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at = at.saturating_add(1);
        }
        let start = at;
        while bytes.get(at).is_some_and(|b| *b != b'=' && !b.is_ascii_whitespace()) {
            at = at.saturating_add(1);
        }
        let Some(name) = text.get(start..at) else { break };
        if name.is_empty() {
            break;
        }
        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at = at.saturating_add(1);
        }
        if bytes.get(at) != Some(&b'=') {
            continue;
        }
        at = at.saturating_add(1);
        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at = at.saturating_add(1);
        }
        let Some(&quote) = bytes.get(at) else { break };
        if quote != b'"' && quote != b'\'' {
            break;
        }
        at = at.saturating_add(1);
        let from = at;
        while bytes.get(at).is_some_and(|b| *b != quote) {
            at = at.saturating_add(1);
        }
        let Some(value) = text.get(from..at) else { break };
        at = at.saturating_add(1);
        out.push((name.to_string(), entities(value)));
    }
    out
}

/// The five named entities XML has, and a numeric reference.
fn entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let Some((name, after)) = from.get(1..).and_then(|f| f.split_once(';')) else {
            out.push_str(from);
            return out;
        };
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // The PNG fixtures are written by an encoder that is not this one: Python's
    // `zlib` at level 9 over hand-built chunks. So what they prove is that this
    // reads what the world writes, rather than what `encode` writes.

    /// An 8x8 RGB image, every row through filter 0.
    const FILTER_0: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAA00lEQVR42gHIADf/AAAAAB8FAT4KAl0PA3wUBJsZBboeBtkjBwAHPQEmQgJFRwNkTASDUQWiVgbBWwfgYAgADnoCLX8DTIQEa4kFio4GqZMHyJgI550JABW3AzS8BFPBBXLGBpHLB7DQCM/VCe7aCgAc9AQ7+QVa/gZ5AweYCAi3DQnWEgr1FwsAIzEFQjYGYTsHgEAIn0UJvkoK3U8L/FQMACpuBklzB2h4CId9CaaCCsWHC+SMDAORDQAxqwdQsAhvtQmOugqtvwvMxAzryQ0Kzg6wwz3BVsdyYwAAAABJRU5ErkJggg==";

    /// An 8x8 RGB image, every row through filter 1.
    const FILTER_1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAANklEQVR42mNkYGCQZ2XERIzstozYJfiqmLBLiG5nxi4h84UFu4SyISt2Ca08NuwShqvZsUoAAL/oDK0LGvr7AAAAAElFTkSuQmCC";

    /// An 8x8 RGB image, every row through filter 2.
    const FILTER_2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAKUlEQVR42mNiYGCQZ2W042KK5WeuEWGZLcm6S47tpjI7E7stI1Y0OCUAGigTNchQLPcAAAAASUVORK5CYII=";

    /// An 8x8 RGB image, every row through filter 3.
    const FILTER_3: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAT0lEQVR42mNmYGCQZ2XU52Cy42Ly42WO5WfOFWKpEWFhZrdlFFbEgpi5Y5iwS/BV4ZAQmsmM4C9EIGbR7cxQDpoOyTAW7EbJlGCRmKzICACE2xQGWKneNQAAAABJRU5ErkJggg==";

    /// An 8x8 RGB image, every row through filter 4.
    const FILTER_4: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAHklEQVR42mNhYGCQZ2XERCzstozsrFgQbSRsKTQKAGoqBbyMRzRuAAAAAElFTkSuQmCC";

    /// A 12x12 RGBA image, deflated by zlib at level 9 — a dynamic block.
    const DYNAMIC: &str = "iVBORw0KGgoAAAANSUhEUgAAAAwAAAAMCAYAAABWdVznAAACLklEQVR42gXBCyyUAQDA8a8LQyVSlnF1Wx5ZSFm3mNJDJq+RSulxdkbuljAVypTNkkSt8ugxlU2K45rPI2FpE0VLGaWEMqmdpvJoCP9+PyHV4jMD9no4WkcymVfKJevV/FOWs7JDSnx5GPsTP/HB9RvNzRrKtVYIg5WrSJArcFGZ8+hmHM5WyXS0HOfwjzki7U1xb4ARwRIfZy1fu8IRDJRSWpv0mBjWIO2v4lqBlIgiCeGp67D09Ka/LIlnCxK4LHOnWxxD0JUM0DujIKmuguAntdQcMKRWdRCbyXdkXxxlh68bITpTrnq2kynzQch5mYu2p5qG5tfs8sjk1oAD7UFmPIyD56EGRB2qocRZyd5fzsxVDCCMtXqw02SaJs0mprfMsTXej7t+N/jZtZbqpjZ8I4yol6TQE+LFBlFEsFBLyF96lPTuPGKCFRSZibia6Pj9IgZVvobR4i6uhDlhHZyG7fYTCDMbVZSZajGM+ML9tmFa0jPIWOxPVckd+mL12GP/nTW6TmRZcj46HkEojHLknH44/m6vyG3UURo4we0zyWwu/ItZ4QVOl90j0XaEocjrBBjbIUiMhjAvUhI0u4zd890EikuI9W8nJVtEX55IWM0KsrapidZXk+bkjmDXH82xsKcsN82hUdZLZc1JFsoNOW/pTd2iQuazxlGsf4MmQORUqTGCsXqKs8q3FBT08ScpmvfW9YSWTeH1QIGTOIPSxYbx4jEeD3bisG+W/0xvZSE4xFeHAAAAAElFTkSuQmCC";

    /// Four one-bit grey pixels.
    const GREY1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABAQAAAADRRzJgAAAACklEQVR42mMIAAAAUgBRWqmjOgAAAABJRU5ErkJggg==";

    /// Four two-bit grey pixels.
    const GREY2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABAgAAAACW50iwAAAACklEQVR42mOQBgAAHQAcI3yPrAAAAABJRU5ErkJggg==";

    /// Four four-bit grey pixels.
    const GREY4: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABBAAAAAAZp70QAAAAC0lEQVR42mNgXQ8AALwAtRHl9XsAAAAASUVORK5CYII=";

    /// Four eight-bit grey pixels.
    const GREY8: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAAAAADcV1ARAAAADUlEQVR42mNgCF31HwADVwH/8jhg/wAAAABJRU5ErkJggg==";

    /// Four sixteen-bit grey pixels.
    const GREY16: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABEAAAAACMx4xSAAAAEUlEQVR42mNgYAgNXbXq/38AC1MD/epjD7sAAAAASUVORK5CYII=";

    /// Two eight-bit RGB pixels.
    const RGB8: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAAD0lEQVR42mP4z8DA0PAfAAgAAn8lPvwJAAAAAElFTkSuQmCC";

    /// Two sixteen-bit RGB pixels.
    const RGB16: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABEAIAAAAr0DSeAAAAEklEQVR42mP4/58BDBoa/v8HAB1zBP3kwgJ1AAAAAElFTkSuQmCC";

    /// Two eight-bit grey-and-alpha pixels.
    const GREYA8: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAQAAABeK7cBAAAADUlEQVR42mNI+X+CAQAGIgIs1WyWQwAAAABJRU5ErkJggg==";

    /// Two sixteen-bit grey-and-alpha pixels.
    const GREYA16: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABEAQAAAAOu2tCAAAAEUlEQVR42mNISfn//8QJBgYAFlIEV24gYSEAAAAASUVORK5CYII=";

    /// One sixteen-bit RGBA pixel.
    const RGBA16: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABEAYAAABPhRjKAAAAEUlEQVR42mMQMgmrmLXn3gcADakEOS7vVqQAAAAASUVORK5CYII=";

    /// Four eight-bit palette indices over a three-entry palette.
    const PAL8: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAMAAADO4v//AAAACVBMVEX/AAAA/wAAAP8tSs2KAAAADUlEQVR42mNgYGRiAAAADAAEFsaHgwAAAABJRU5ErkJggg==";

    /// The same four pixels at four bits.
    const PAL4: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABBAMAAAALEhL+AAAACVBMVEX/AAAA/wAAAP8tSs2KAAAAC0lEQVR42mNgVAAAACUAIumCh+UAAAAASUVORK5CYII=";

    /// Four one-bit palette indices.
    const PAL1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABAQMAAADD8p2OAAAABlBMVEX/AAAA/wDSh+9xAAAACklEQVR42mNIAAAAYgBhHBADfwAAAABJRU5ErkJggg==";

    /// Grey with one sample declared transparent.
    const TRNS_GREY: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAAAAADcV1ARAAAAAnRSTlMAVW2SaEMAAAANSURBVHjaY2AIXfUfAANXAf/yOGD/AAAAAElFTkSuQmCC";

    /// RGB with one colour declared transparent.
    const TRNS_RGB: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAIAAAB7QOjdAAAABnRSTlMAAACAAP+ikfOcAAAAD0lEQVR42mP4z8DA0PAfAAgAAn8lPvwJAAAAAElFTkSuQmCC";

    /// A palette whose first two entries carry an alpha.
    const TRNS_PAL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAMAAADO4v//AAAACVBMVEX/AAAA/wAAAP8tSs2KAAAAAnRSTlMAgJsrThgAAAANSURBVHjaY2BgZGIAAAAMAAQWxoeDAAAAAElFTkSuQmCC";

    /// A file whose header claims Adam7.
    const INTERLACED: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAEBWagMAAAAFUlEQVR42mNgZGJmYWVj5+Dk4uYBAAF5AE9M9yZTAAAAAElFTkSuQmCC";

    fn png(text: &str) -> Image {
        decode(&base64(text).expect("the fixture is base64")).expect("the fixture is a PNG")
    }

    /// The pixels of an image, as `[r, g, b, a]` each.
    fn rgba(image: &Image) -> Vec<[u8; 4]> {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .map(|(x, y)| image.pixel(x, y).expect("a pixel inside the image"))
            .collect()
    }

    // -- inflate -------------------------------------------------------------

    /// A block a real encoder chose, which is the one this reader could not
    /// read at all before: `zlib` picks dynamic Huffman for anything with
    /// enough entropy in it, and every PNG on the web is one.
    #[test]
    fn a_dynamic_huffman_block_inflates() {
        let image = png(DYNAMIC);
        assert_eq!((image.width, image.height), (12, 12));
        let mut expected: Vec<u8> = Vec::new();
        let mut state: u64 = 12345;
        for _ in 0..12 * 12 {
            state = (state.wrapping_mul(1_103_515_245).wrapping_add(12345)) & 0x7fff_ffff;
            expected.extend_from_slice(&[
                (state & 0xff) as u8,
                ((state >> 8) & 0xff) as u8,
                ((state >> 16) & 0xff) as u8,
                255,
            ]);
        }
        assert_eq!(image.rgba, expected);
    }

    #[test]
    fn a_stream_that_is_not_deflate_is_refused() {
        assert!(inflate(&[0x79, 0x01]).is_err());
        assert!(inflate(&[0x78]).is_err());
        // The preset-dictionary bit: a stream whose first bytes this cannot use.
        assert!(inflate(&[0x78, 0xa0, 0x00]).is_err());
    }

    // -- the row filters -----------------------------------------------------

    /// One image, written five times, once through each filter. They all read
    /// back as the same pixels or one of the five is wrong.
    #[test]
    fn every_row_filter_is_undone() {
        let first = rgba(&png(FILTER_0));
        assert_eq!(first.len(), 64);
        assert_eq!(first.first(), Some(&[0, 0, 0, 255]));
        // The pixel at (1, 1), which the generator wrote as (31+7, 5+61, 2).
        assert_eq!(first.get(9), Some(&[38, 66, 2, 255]));
        for (kind, text) in [(1, FILTER_1), (2, FILTER_2), (3, FILTER_3), (4, FILTER_4)] {
            assert_eq!(rgba(&png(text)), first, "filter {kind} did not come back");
        }
    }

    #[test]
    fn a_row_filter_that_does_not_exist_is_refused() {
        let mut row = vec![0_u8; 4];
        assert!(unfilter(5, &mut row, &[0; 4], 4).is_err());
    }

    // -- the colour types ----------------------------------------------------

    #[test]
    fn grey_reads_at_every_depth_it_has() {
        let ramp = |v: u8| [v, v, v, 255];
        for text in [GREY1, GREY2, GREY4, GREY8, GREY16] {
            let read = rgba(&png(text));
            let expected = match text {
                t if t == GREY1 => vec![ramp(0), ramp(255), ramp(0), ramp(255)],
                _ => vec![ramp(0), ramp(85), ramp(170), ramp(255)],
            };
            assert_eq!(read, expected);
        }
    }

    #[test]
    fn colour_reads_at_both_depths() {
        assert_eq!(rgba(&png(RGB8)), vec![[255, 0, 0, 255], [0, 128, 255, 255]]);
        assert_eq!(rgba(&png(RGB16)), vec![[255, 0, 0, 255], [0, 128, 255, 255]]);
        assert_eq!(rgba(&png(RGBA16)), vec![[0x12, 0x56, 0x9a, 0xde]]);
    }

    #[test]
    fn grey_with_an_alpha_channel_reads_at_both_depths() {
        assert_eq!(rgba(&png(GREYA8)), vec![[100, 100, 100, 255], [200, 200, 200, 0]]);
        assert_eq!(rgba(&png(GREYA16)), vec![[100, 100, 100, 255], [200, 200, 200, 0]]);
    }

    #[test]
    fn a_palette_reads_at_every_depth_it_has() {
        let (red, green, blue) = ([255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]);
        assert_eq!(rgba(&png(PAL8)), vec![red, green, blue, red]);
        assert_eq!(rgba(&png(PAL4)), vec![red, green, blue, red]);
        assert_eq!(rgba(&png(PAL1)), vec![red, green, green, red]);
    }

    /// All three forms of `tRNS`: a grey sample, a colour, and one alpha per
    /// palette entry.
    #[test]
    fn transparency_reads_in_all_three_forms() {
        let clear = rgba(&png(TRNS_GREY));
        assert_eq!(clear.get(1), Some(&[85, 85, 85, 0]));
        assert_eq!(clear.get(2), Some(&[170, 170, 170, 255]));
        assert_eq!(rgba(&png(TRNS_RGB)), vec![[255, 0, 0, 255], [0, 128, 255, 0]]);
        let palette = rgba(&png(TRNS_PAL));
        assert_eq!(palette.first(), Some(&[255, 0, 0, 0]));
        assert_eq!(palette.get(1), Some(&[0, 255, 0, 128]));
        assert_eq!(palette.get(2), Some(&[0, 0, 255, 255]));
    }

    /// Adam7 is the one thing the reader refuses, and it says so.
    #[test]
    fn an_interlaced_png_is_refused() {
        let error = decode(&base64(INTERLACED).unwrap()).unwrap_err();
        assert!(error.contains("interlaced"), "{error}");
        assert!(read(&format!("data:image/png;base64,{INTERLACED}")).is_none());
    }

    #[test]
    fn something_that_is_not_a_png_is_refused() {
        assert!(decode(b"not a png at all").is_err());
        assert!(read("https://example.com/logo.png").is_none());
        assert!(read("assets/logo.png").is_none());
        assert!(read("").is_none());
    }

    // -- SVG -----------------------------------------------------------------

    /// Draws a document into a canvas of `size` pixels, over white.
    fn drawn(document: &str, size: u32) -> Pixmap {
        let source = format!("data:image/svg+xml,{document}");
        let Some(Picture::Vector(svg)) = read(&source) else {
            panic!("`{document}` did not read as a vector");
        };
        let mut canvas = Pixmap::new(size, size).expect("a canvas");
        canvas.fill(tiny_skia::Color::WHITE);
        let box_ = Box2 { l: 0, t: 0, r: size as i32, b: size as i32 };
        svg.draw(&mut canvas, box_, 1.0, Rgba { r: 0, g: 0, b: 255, a: 1.0 }, None);
        canvas
    }

    fn at(canvas: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = canvas.pixel(x, y).expect("a pixel inside the canvas").demultiply();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    /// A `viewBox` of ten in a canvas of twenty, so one user unit is two
    /// pixels and every assertion below is in the middle of a cell.
    fn box10(body: &str) -> Pixmap {
        drawn(&format!("<svg viewBox='0 0 10 10'>{body}</svg>"), 20)
    }

    #[test]
    fn a_rect_fills_the_corner_it_names() {
        let canvas = box10("<rect x='0' y='0' width='5' height='5' fill='red'/>");
        assert_eq!(at(&canvas, 5, 5), RED);
        assert_eq!(at(&canvas, 15, 15), WHITE);
    }

    /// The corner an `rx` rounds off is the one pixel that says it happened.
    #[test]
    fn a_rounded_rect_leaves_its_corner_empty() {
        let square = box10("<rect x='0' y='0' width='10' height='10' fill='red'/>");
        let rounded = box10("<rect x='0' y='0' width='10' height='10' rx='4' fill='red'/>");
        assert_eq!(at(&square, 0, 0), RED);
        assert_eq!(at(&rounded, 0, 0), WHITE);
        assert_eq!(at(&rounded, 10, 10), RED);
    }

    #[test]
    fn a_circle_and_an_ellipse_fill_their_middles() {
        let circle = box10("<circle cx='5' cy='5' r='4' fill='red'/>");
        assert_eq!(at(&circle, 10, 10), RED);
        assert_eq!(at(&circle, 1, 1), WHITE);
        let ellipse = box10("<ellipse cx='5' cy='5' rx='4' ry='1' fill='red'/>");
        assert_eq!(at(&ellipse, 10, 10), RED);
        assert_eq!(at(&ellipse, 10, 16), WHITE);
        assert_eq!(at(&ellipse, 4, 10), RED);
    }

    #[test]
    fn a_line_is_stroked_and_not_filled() {
        let canvas = box10("<line x1='0' y1='5' x2='10' y2='5' stroke='red' stroke-width='2'/>");
        assert_eq!(at(&canvas, 10, 10), RED);
        assert_eq!(at(&canvas, 10, 2), WHITE);
    }

    #[test]
    fn a_polyline_stays_open_and_a_polygon_closes() {
        let points = "points='1,1 9,1 9,9'";
        let open = box10(&format!("<polyline {points} fill='none' stroke='red'/>"));
        let closed = box10(&format!("<polygon {points} fill='none' stroke='red'/>"));
        // The leg from the last point back to the first is the difference.
        assert_eq!(at(&open, 10, 10), WHITE);
        assert_eq!(at(&closed, 10, 10), RED);
    }

    /// One assertion per path command, each drawn so that a command that did
    /// nothing leaves the pixel white.
    #[test]
    fn every_path_command_draws() {
        let stroke = "fill='none' stroke='red' stroke-width='2'";
        for (name, d, x, y) in [
            ("M and L", "M 0 5 L 10 5", 10, 10),
            ("m and l", "m 0 5 l 10 0", 10, 10),
            ("H and V", "M 0 5 H 10 V 10", 18, 18),
            ("h and v", "M 0 5 h 10 v 5", 18, 18),
            ("C", "M 0 5 C 3 0, 7 0, 10 5", 10, 4),
            ("c", "M 0 5 c 3 -5, 7 -5, 10 0", 10, 4),
            ("S", "M 0 5 C 1 5, 2 5, 3 5 S 8 0, 10 0", 18, 2),
            ("Q", "M 0 5 Q 5 0, 10 5", 10, 5),
            ("q", "M 0 5 q 5 -5, 10 0", 10, 5),
            ("T", "M 0 5 Q 2 0, 4 5 T 10 5", 14, 13),
            ("A", "M 1 5 A 4 4 0 0 1 9 5", 10, 3),
            ("a", "M 1 5 a 4 4 0 0 1 8 0", 10, 3),
        ] {
            let canvas = box10(&format!("<path d='{d}' {stroke}/>"));
            assert_ne!(at(&canvas, x, y), WHITE, "`{name}` drew nothing at {x},{y}");
        }
        // `Z` closes back to the subpath's start, which is the third side.
        let open = box10(&format!("<path d='M 1 1 L 9 1 L 9 9' {stroke}/>"));
        let closed = box10(&format!("<path d='M 1 1 L 9 1 L 9 9 Z' {stroke}/>"));
        assert_eq!(at(&open, 10, 10), WHITE);
        assert_ne!(at(&closed, 10, 10), WHITE);
    }

    #[test]
    fn a_group_hands_down_its_paint_and_its_transform() {
        let canvas =
            box10("<g fill='red' transform='translate(5 0)'><rect width='5' height='5'/></g>");
        assert_eq!(at(&canvas, 15, 5), RED);
        assert_eq!(at(&canvas, 5, 5), WHITE);
    }

    #[test]
    fn every_transform_moves_what_it_says() {
        for (name, list, x, y) in [
            ("translate", "translate(5,5)", 15, 15),
            ("scale", "scale(2)", 5, 5),
            ("rotate", "rotate(90 5 5)", 15, 5),
            ("matrix", "matrix(1 0 0 1 5 5)", 15, 15),
            ("two of them", "translate(5 0) translate(0 5)", 15, 15),
        ] {
            let canvas =
                box10(&format!("<rect width='5' height='5' fill='red' transform='{list}'/>"));
            assert_eq!(at(&canvas, x, y), RED, "`{name}` did not move the rect");
        }
    }

    #[test]
    fn current_colour_is_the_colour_the_element_carries() {
        let canvas = box10("<rect width='10' height='10' fill='currentColor'/>");
        assert_eq!(at(&canvas, 10, 10), [0, 0, 255, 255]);
    }

    #[test]
    fn none_paints_nothing() {
        let canvas = box10("<rect width='10' height='10' fill='none'/>");
        assert_eq!(at(&canvas, 10, 10), WHITE);
    }

    #[test]
    fn every_colour_form_reads() {
        for form in ["#f00", "#ff0000", "#ff0000ff", "rgb(255,0,0)", "rgba(255, 0, 0, 1)", "red"] {
            let canvas = box10(&format!("<rect width='10' height='10' fill='{form}'/>"));
            assert_eq!(at(&canvas, 10, 10), RED, "`{form}` did not read as red");
        }
    }

    /// The three opacities, each halving what is under it against white.
    #[test]
    fn the_opacities_multiply_into_the_colour() {
        for attribute in ["opacity='0.5'", "fill-opacity='0.5'"] {
            let canvas = box10(&format!("<rect width='10' height='10' fill='red' {attribute}/>"));
            assert_eq!(at(&canvas, 10, 10), [255, 127, 127, 255], "with {attribute}");
        }
        let canvas =
            box10("<line x1='0' y1='5' x2='10' y2='5' stroke='red' stroke-width='4' \
                   stroke-opacity='0.5'/>");
        assert_eq!(at(&canvas, 10, 10), [255, 127, 127, 255]);
    }

    #[test]
    fn a_round_cap_reaches_past_the_end_a_butt_cap_stops_at() {
        let butt = box10("<line x1='2' y1='5' x2='8' y2='5' stroke='red' stroke-width='4'/>");
        let round = box10(
            "<line x1='2' y1='5' x2='8' y2='5' stroke='red' stroke-width='4' \
             stroke-linecap='round'/>",
        );
        assert_eq!(at(&butt, 2, 10), WHITE);
        assert_eq!(at(&round, 2, 10), RED);
    }

    /// What the subset does not draw is skipped, and the shape beside it still
    /// paints — which is the whole promise of "paints what it can".
    #[test]
    fn what_is_outside_the_subset_stops_nothing() {
        let canvas = box10(
            "<defs><linearGradient id='g'><stop offset='0'/></linearGradient></defs>\
             <text x='0' y='5'>ignored</text>\
             <rect width='10' height='10' fill='red'/>",
        );
        assert_eq!(at(&canvas, 10, 10), RED);
        // A `defs` child is a definition and not a picture, so it draws nothing
        // even though it is a shape.
        let hidden = box10("<defs><rect width='10' height='10' fill='red'/></defs>");
        assert_eq!(at(&hidden, 10, 10), WHITE);
    }

    #[test]
    fn a_comment_and_a_declaration_are_skipped() {
        let canvas = drawn(
            "<?xml version='1.0'?><!-- a comment --><svg viewBox='0 0 10 10'>\
             <rect width='10' height='10' fill='red'/></svg>",
            20,
        );
        assert_eq!(at(&canvas, 10, 10), RED);
    }

    /// A percent-encoded body is the other form a page writes an SVG in, and
    /// `//libs/ui/icons` writes every glyph that way.
    #[test]
    fn a_percent_encoded_body_reads_like_a_base64_one() {
        let body = "%3Csvg viewBox='0 0 10 10'%3E%3Crect width='10' height='10' \
                    fill='%23ff0000'/%3E%3C/svg%3E";
        let Some(Picture::Vector(svg)) = read(&format!("data:image/svg+xml;charset=utf-8,{body}"))
        else {
            panic!("a percent-encoded SVG did not read");
        };
        let mut canvas = Pixmap::new(20, 20).unwrap();
        canvas.fill(tiny_skia::Color::WHITE);
        svg.draw(&mut canvas, Box2 { l: 0, t: 0, r: 20, b: 20 }, 1.0, Rgba::BLACK, None);
        assert_eq!(at(&canvas, 10, 10), RED);
    }

    // -- the size a picture takes -------------------------------------------

    #[test]
    fn a_pngs_own_pixels_are_its_size() {
        let source = format!("data:image/png;base64,{RGB8}");
        assert_eq!(read(&source).map(|p| p.intrinsic()), Some((2.0, 1.0)));
    }

    #[test]
    fn an_svg_takes_its_width_and_height_and_falls_back_to_its_view_box() {
        let sized = "data:image/svg+xml,<svg width='40' height='20' viewBox='0 0 10 10'/>";
        assert_eq!(read(sized).map(|p| p.intrinsic()), Some((40.0, 20.0)));
        let boxed = "data:image/svg+xml,<svg viewBox='0 0 16 24'/>";
        assert_eq!(read(boxed).map(|p| p.intrinsic()), Some((16.0, 24.0)));
        let neither = "data:image/svg+xml,<svg><rect width='4' height='4'/></svg>";
        assert!(read(neither).is_none());
    }

    /// The box wins where it declares a size, and an SVG is re-rasterized into
    /// it rather than scaled up from its own units.
    #[test]
    fn a_sized_box_scales_the_picture_into_it() {
        let source = "data:image/svg+xml,<svg viewBox='0 0 2 2'>\
                      <rect width='1' height='2' fill='red'/></svg>";
        let Some(picture) = read(source) else { panic!("the SVG did not read") };
        let mut canvas = Pixmap::new(40, 40).unwrap();
        canvas.fill(tiny_skia::Color::WHITE);
        picture.draw(&mut canvas, Box2 { l: 0, t: 0, r: 40, b: 40 }, 1.0, Rgba::BLACK, None);
        assert_eq!(at(&canvas, 5, 20), RED);
        assert_eq!(at(&canvas, 35, 20), WHITE);
    }

    // -- the program in the issue -------------------------------------------

    /// buri-lang/buri#85's own tree, at the scene `describe` writes for it: a
    /// red PNG square beside a red SVG plus sign, which both painted as the
    /// same grey box. The PNG is colour type 2 at eight bits, which is not what
    /// `encode` writes; the SVG is two stroked paths in a `viewBox`.
    #[test]
    fn the_two_icons_in_the_issue_paint_a_red_square_and_a_red_plus() {
        let png = "data:image/png\\;base64,iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8CAFWEXHbQSACj/P8Fu7N9hAAAAAElFTkSuQmCC";
        let svg = "data:image/svg+xml\\;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHdpZHRoPScyNCcgaGVpZ2h0PScyNCcgdmlld0JveD0nMCAwIDI0IDI0JyBmaWxsPSdub25lJyBzdHJva2U9JyNlNzAwMDAnIHN0cm9rZS13aWR0aD0nMic+PHBhdGggZD0nTTUgMTJoMTQnLz48cGF0aCBkPSdNMTIgNXYxNCcvPjwvc3ZnPg==";
        let scene = format!(
            "buri-scene 1\nviewport 800 600\n\
             e 0 display:flex;flex-direction:row;gap:12px;padding:16px\n\
             e 1 width:24px;height:24px\n\
             e 2 image:{png}\n\
             e 1 width:24px;height:24px\n\
             e 2 image:{svg}\n"
        );
        let request = super::super::Request {
            scene: &scene,
            stylesheet: "",
            state: "checked",
            variables: "",
        };
        let bytes = super::super::render(&request).expect("the scene paints");
        let image = decode(&bytes).expect("the painter's own PNG");
        let pixel = |x, y| image.pixel(x, y).expect("a pixel inside the page");

        // The PNG sits at its own eight pixels in the corner of its box.
        assert_eq!(pixel(20, 20), RED);
        assert_eq!(pixel(30, 30), WHITE);
        // The plus is two strokes in its own colour, with page between them.
        assert_eq!(pixel(64, 28), [231, 0, 0, 255]);
        assert_eq!(pixel(64, 22), [231, 0, 0, 255]);
        assert_eq!(pixel(58, 22), WHITE);
        assert_eq!(pixel(46, 28), WHITE);
    }
}
