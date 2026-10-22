// SPDX-License-Identifier: CC0-1.0

//! Mapbox Vector Tile encoding for `GetTile`'s vector tilesets.
//!
//! The message definitions below are the `vector_tile.proto` schema of the
//! Mapbox Vector Tile specification 2.1 (the format AWS serves for the
//! `vector.*` tilesets), transcribed as `prost` messages: the protobuf wire
//! codec itself is `prost`'s, not hand-written, so the tiles this service
//! serves decode in any MVT reader.
//!
//! What is written here is the part of the format that is not a codec: the
//! command-and-parameter geometry encoding (§4.3 of the specification) and the
//! key/value interning a layer's attributes use.

/// A whole tile: `repeated Layer layers = 3`.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Tile {
    #[prost(message, repeated, tag = "3")]
    pub layers: Vec<Layer>,
}

/// One layer of a tile.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Layer {
    /// `required uint32 version = 15` — 2 for specification 2.x.
    #[prost(uint32, tag = "15")]
    pub version: u32,
    #[prost(string, tag = "1")]
    pub name: String,
    #[prost(message, repeated, tag = "2")]
    pub features: Vec<Feature>,
    #[prost(string, repeated, tag = "3")]
    pub keys: Vec<String>,
    #[prost(message, repeated, tag = "4")]
    pub values: Vec<Value>,
    /// The layer's coordinate extent; 4096 is the specification's default.
    #[prost(uint32, tag = "5")]
    pub extent: u32,
}

/// One feature: its attribute indices and its encoded geometry.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Feature {
    #[prost(uint64, tag = "1")]
    pub id: u64,
    /// Pairs of (key index, value index) into the layer's `keys`/`values`.
    #[prost(uint32, repeated, tag = "2")]
    pub tags: Vec<u32>,
    #[prost(enumeration = "GeomType", tag = "3")]
    pub r#type: i32,
    /// The command-and-parameter stream (see [`GeometryWriter`]).
    #[prost(uint32, repeated, tag = "4")]
    pub geometry: Vec<u32>,
}

/// An attribute value. The specification makes every variant optional and
/// exactly one is set per value.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Value {
    #[prost(string, optional, tag = "1")]
    pub string_value: Option<String>,
    #[prost(float, optional, tag = "2")]
    pub float_value: Option<f32>,
    #[prost(double, optional, tag = "3")]
    pub double_value: Option<f64>,
    #[prost(int64, optional, tag = "4")]
    pub int_value: Option<i64>,
    #[prost(uint64, optional, tag = "5")]
    pub uint_value: Option<u64>,
    #[prost(sint64, optional, tag = "6")]
    pub sint_value: Option<i64>,
    #[prost(bool, optional, tag = "7")]
    pub bool_value: Option<bool>,
}

/// `Tile.GeomType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum GeomType {
    Unknown = 0,
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

/// The geometry commands the specification defines.
const CMD_MOVE_TO: u32 = 1;
const CMD_LINE_TO: u32 = 2;
const CMD_CLOSE_PATH: u32 = 7;

/// Builds a feature's command-and-parameter geometry stream.
///
/// Coordinates are layer-extent integers and every command's parameters are
/// deltas from the cursor, zig-zag encoded — §4.3.2 of the specification.
#[derive(Debug, Default)]
pub struct GeometryWriter {
    stream: Vec<u32>,
    cursor_x: i32,
    cursor_y: i32,
}

impl GeometryWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `MoveTo` the given extent coordinate.
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.stream.push(command(CMD_MOVE_TO, 1));
        self.push_delta(x, y);
    }

    /// `LineTo` each of `points`, in order. A no-op for an empty slice, which
    /// keeps a zero-length command out of the stream.
    pub fn line_to(&mut self, points: &[(i32, i32)]) {
        if points.is_empty() {
            return;
        }
        self.stream
            .push(command(CMD_LINE_TO, u32::try_from(points.len()).unwrap_or(0)));
        for &(x, y) in points {
            self.push_delta(x, y);
        }
    }

    /// `ClosePath`, which takes no parameters.
    pub fn close_path(&mut self) {
        self.stream.push(command(CMD_CLOSE_PATH, 1));
    }

    /// The finished stream.
    pub fn finish(self) -> Vec<u32> {
        self.stream
    }

    fn push_delta(&mut self, x: i32, y: i32) {
        let dx = x.wrapping_sub(self.cursor_x);
        let dy = y.wrapping_sub(self.cursor_y);
        self.stream.push(zigzag(dx));
        self.stream.push(zigzag(dy));
        self.cursor_x = x;
        self.cursor_y = y;
    }
}

/// A command integer: the command id in the low three bits, the repeat count
/// above them. The count is capped at the 29 bits the layout leaves for it, so
/// a count that cannot be represented truncates to the maximum rather than
/// wrapping into the command id and producing a malformed stream.
fn command(id: u32, count: u32) -> u32 {
    const MAX_COUNT: u32 = (1 << 29) - 1;
    (id & 0x7) | (count.min(MAX_COUNT) << 3)
}

/// Zig-zag encoding, so a small negative delta stays a small varint.
fn zigzag(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

/// An attribute value a feature carries.
#[derive(Debug, Clone, PartialEq)]
pub enum TagValue {
    Str(String),
    Int(i64),
    Double(f64),
}

/// Accumulates one layer, interning attribute keys and values the way the
/// specification requires (a feature stores indices, not strings).
#[derive(Debug)]
pub struct LayerBuilder {
    name: String,
    extent: u32,
    features: Vec<Feature>,
    keys: Vec<String>,
    values: Vec<Value>,
}

impl LayerBuilder {
    pub fn new(name: &str, extent: u32) -> Self {
        Self {
            name: name.to_string(),
            extent,
            features: Vec::new(),
            keys: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Append a feature with `attributes`, interning each key and value.
    pub fn add_feature(
        &mut self,
        id: u64,
        geom_type: GeomType,
        geometry: Vec<u32>,
        attributes: &[(&str, TagValue)],
    ) {
        let mut tags = Vec::with_capacity(attributes.len() * 2);
        for (key, value) in attributes {
            tags.push(self.intern_key(key));
            tags.push(self.intern_value(value));
        }
        self.features.push(Feature {
            id,
            tags,
            r#type: geom_type as i32,
            geometry,
        });
    }

    /// The finished layer.
    pub fn finish(self) -> Layer {
        Layer {
            // Specification 2.x layers declare version 2.
            version: 2,
            name: self.name,
            features: self.features,
            keys: self.keys,
            values: self.values,
            extent: self.extent,
        }
    }

    fn intern_key(&mut self, key: &str) -> u32 {
        if let Some(index) = self.keys.iter().position(|existing| existing == key) {
            return index as u32;
        }
        self.keys.push(key.to_string());
        (self.keys.len() - 1) as u32
    }

    fn intern_value(&mut self, value: &TagValue) -> u32 {
        let mut encoded = Value::default();
        match value {
            TagValue::Str(text) => encoded.string_value = Some(text.clone()),
            TagValue::Int(number) => encoded.int_value = Some(*number),
            TagValue::Double(number) => encoded.double_value = Some(*number),
        }
        if let Some(index) = self.values.iter().position(|existing| *existing == encoded) {
            return index as u32;
        }
        self.values.push(encoded);
        (self.values.len() - 1) as u32
    }
}
