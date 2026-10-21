// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! [`Codec`] implementation built on `aws-smithy-json`.
//!
//! `aws-smithy-json` is a low-level JSON tokeniser/writer; it does not know
//! about Smithy schemas. This module bridges the two: a [`JsonShapeSerializer`]
//! / [`JsonShapeDeserializer`] pair that adapts `aws-smithy-json`'s API to the
//! [`ShapeSerializer`] / [`ShapeDeserializer`] traits from `aws-smithy-schema`.
//!
//! Together with [`JsonCodec`], this lets the awsJson family of protocols
//! plug straight into [`HttpRpcProtocol`][rpc] /
//! [`HttpRpcServerProtocol`][crate::rpc::HttpRpcServerProtocol] without each
//! protocol having to re-implement struct/list/map walk-down code.
//!
//! [rpc]: https://docs.rs/aws-smithy-schema/0.1.0/aws_smithy_schema/http_protocol/struct.HttpRpcProtocol.html
//!
//! # Number representation
//!
//! Smithy distinguishes `byte` / `short` / `integer` / `long` / `float` /
//! `double` / `bigInteger` / `bigDecimal`. JSON has only "number". The
//! deserializer accepts any in-range JSON number for the target Smithy
//! integer type and any number for the target float/double type. Big
//! integers and decimals are read directly from the raw number characters in
//! the JSON source to preserve precision.

use aws_smithy_json::deserialize::error::DeserializeError;
use aws_smithy_json::deserialize::token::{expect_number_or_null, EscapedStr, Token};
use aws_smithy_json::deserialize::{json_token_iter, JsonTokenIterator};
use aws_smithy_json::serialize::JsonValueWriter;
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{base64, BigDecimal, BigInteger, Blob, DateTime, Document, Number};
use std::iter::Peekable;
use std::str::FromStr;

/// Map a Smithy `@timestampFormat` to the runtime `aws_smithy_types`
/// [`Format`] used by the JSON writer/parser.
fn timestamp_format(schema_format: Option<TimestampFormat>) -> Format {
    match schema_format {
        Some(TimestampFormat::DateTime) => Format::DateTime,
        Some(TimestampFormat::HttpDate) => Format::HttpDate,
        Some(TimestampFormat::EpochSeconds) | None => Format::EpochSeconds,
    }
}

/// JSON codec — produces [`JsonShapeSerializer`] and [`JsonShapeDeserializer`].
///
/// This is the codec used by `awsJson1_0`, `awsJson1_1`, and `restJson1`.
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonCodec {
    /// When `true`, the codec honors `@jsonName` on members (restJson behavior).
    /// When `false`, the wire name is always the Smithy member name (awsJson behavior).
    use_json_name: bool,
}

impl JsonCodec {
    pub const fn new() -> Self {
        Self {
            use_json_name: false,
        }
    }

    /// Create a codec that honors `@jsonName` traits (for restJson protocols).
    pub const fn rest_json() -> Self {
        Self {
            use_json_name: true,
        }
    }
}

impl Codec for JsonCodec {
    type Serializer = JsonShapeSerializer;
    type Deserializer<'a> = JsonShapeDeserializer<'a>;

    fn create_serializer(&self) -> Self::Serializer {
        JsonShapeSerializer::new_with_config(self.use_json_name)
    }

    fn create_deserializer<'a>(&self, input: &'a [u8]) -> Self::Deserializer<'a> {
        JsonShapeDeserializer::new_with_config(input, self.use_json_name)
    }
}

// =================================================================================================
// Serializer
// =================================================================================================

/// JSON [`ShapeSerializer`] backed by `aws-smithy-json`.
pub struct JsonShapeSerializer {
    /// Top-level output buffer.
    output: String,
    /// Stack of in-progress containers. `None` at the bottom means "we are
    /// writing the top-level value". Each `Some` entry remembers the kind of
    /// container we are inside and whether we have already emitted its first
    /// element (so we know when to insert a comma).
    stack: Vec<Frame>,
    /// Whether to honor `@jsonName` on members.
    use_json_name: bool,
}

#[derive(Debug)]
enum Frame {
    /// We are writing inside an object. After [`StartStruct`], every member
    /// emit emits `"name":` first (via [`emit_key`]), then a value.
    Object {
        /// Have we written any member yet (to know whether to emit a comma).
        first: bool,
        /// The most recent key we committed (only meaningful between
        /// `emit_key` and the next value write).
        pending_key: Option<String>,
    },
    /// We are writing inside an array (list).
    Array { first: bool },
    /// We are writing the entries of a map. Maps in JSON are objects, but
    /// the [`ShapeSerializer::write_map`] consumer alternates `key` /
    /// `value` reads on the same serializer, so we need explicit state to
    /// know whether the next scalar/struct write is a key or a value.
    Map { first: bool, expecting_key: bool },
}

impl Default for JsonShapeSerializer {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonShapeSerializer {
    pub fn new() -> Self {
        Self::new_with_config(false)
    }

    pub fn new_with_config(use_json_name: bool) -> Self {
        Self {
            output: String::new(),
            stack: Vec::new(),
            use_json_name,
        }
    }

    pub fn into_string(self) -> String {
        self.output
    }

    /// Emits a `,` if the current container already has at least one element,
    /// then marks the container as non-empty.
    fn emit_separator(&mut self) {
        match self.stack.last_mut() {
            Some(Frame::Object { first, .. }) => {
                if *first {
                    *first = false;
                } else {
                    self.output.push(',');
                }
            }
            Some(Frame::Array { first }) => {
                if *first {
                    *first = false;
                } else {
                    self.output.push(',');
                }
            }
            Some(Frame::Map {
                first,
                expecting_key,
            }) if *expecting_key => {
                // We only emit a comma at the start of each *entry*, i.e.
                // when we are about to write a key.
                if *first {
                    *first = false;
                } else {
                    self.output.push(',');
                }
            }
            None => {}
            _ => {}
        }
    }

    /// Write a JSON value start (the part *before* the actual value bytes).
    /// For objects this means emitting the `"key":` prefix; for everything
    /// else it just emits a separator.
    fn before_value(&mut self) {
        // Maps need special handling: a value might be a "key" (we serialize
        // it as a string, then emit `:`) or a "value" (just emit comma+value).
        if let Some(Frame::Map { expecting_key, .. }) = self.stack.last() {
            if *expecting_key {
                self.emit_separator();
                // The value writer will emit the JSON string for the key.
                return;
            }
            // We're writing a value: emit `:` between key and value, then
            // continue below (no comma here; the comma was emitted before
            // the key).
            self.output.push(':');
            return;
        }
        // Objects need to emit the pending key (if any).
        if let Some(Frame::Object { pending_key, .. }) = self.stack.last_mut() {
            if let Some(key) = pending_key.take() {
                self.emit_separator();
                let writer = JsonValueWriter::new(&mut self.output);
                writer.string(&key);
                self.output.push(':');
                return;
            }
        }
        self.emit_separator();
    }

    /// Called *after* a value is fully written (including end of struct/array).
    fn after_value(&mut self) {
        if let Some(Frame::Map { expecting_key, .. }) = self.stack.last_mut() {
            *expecting_key = !*expecting_key;
        }
    }

    /// Resolve the JSON wire name for a struct member.
    /// For restJson: use @jsonName if present, else member name.
    /// For awsJson: always use the member name.
    fn member_json_name<'a>(&self, member: &'a Schema) -> &'a str {
        if self.use_json_name {
            if let Some(jn) = member.json_name() {
                return jn.value();
            }
        }
        member
            .member_name()
            .expect("struct member schema must have a member name")
    }

    /// Emit a struct member key (used by `serialize_members`) — only valid
    /// when the current frame is an object frame.
    fn emit_member_key(&mut self, member: &Schema) {
        let name = self.member_json_name(member).to_string();
        if let Some(Frame::Object { pending_key, .. }) = self.stack.last_mut() {
            *pending_key = Some(name);
        } else {
            // Defensive: shouldn't happen because the framework only calls
            // member writes from inside `write_struct`, but emit a separator
            // and key inline if it does.
            self.emit_separator();
            let writer = JsonValueWriter::new(&mut self.output);
            writer.string(&name);
            self.output.push(':');
        }
    }
}

impl FinishSerializer for JsonShapeSerializer {
    fn finish(self) -> Vec<u8> {
        self.output.into_bytes()
    }
}

impl ShapeSerializer for JsonShapeSerializer {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        // If this struct is itself a member of a containing struct, the
        // containing serializer has already pushed an `Object` frame and
        // recorded a pending key. `before_value` consumes that key.
        self.before_value();

        // Begin the object.
        self.output.push('{');
        self.stack.push(Frame::Object {
            first: true,
            pending_key: None,
        });

        // Write members. The `serialize_members` impl will call back into
        // this serializer with the member schemas; we use those to emit keys.
        // We thread through a tiny adapter that records the next key.
        struct MemberKeyEmitter<'a> {
            inner: &'a mut JsonShapeSerializer,
        }

        impl<'a> ShapeSerializer for MemberKeyEmitter<'a> {
            fn write_struct(
                &mut self,
                schema: &Schema,
                value: &dyn SerializableStruct,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_struct(schema, value)
            }
            fn write_list(
                &mut self,
                schema: &Schema,
                write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_list(schema, write_elements)
            }
            fn write_map(
                &mut self,
                schema: &Schema,
                write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_map(schema, write_entries)
            }
            fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_boolean(schema, value)
            }
            fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_byte(schema, value)
            }
            fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_short(schema, value)
            }
            fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_integer(schema, value)
            }
            fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_long(schema, value)
            }
            fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_float(schema, value)
            }
            fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_double(schema, value)
            }
            fn write_big_integer(
                &mut self,
                schema: &Schema,
                value: &BigInteger,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_big_integer(schema, value)
            }
            fn write_big_decimal(
                &mut self,
                schema: &Schema,
                value: &BigDecimal,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_big_decimal(schema, value)
            }
            fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_string(schema, value)
            }
            fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_blob(schema, value)
            }
            fn write_timestamp(
                &mut self,
                schema: &Schema,
                value: &DateTime,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_timestamp(schema, value)
            }
            fn write_document(
                &mut self,
                schema: &Schema,
                value: &Document,
            ) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_document(schema, value)
            }
            fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
                self.inner.emit_member_key(schema);
                self.inner.write_null(schema)
            }
        }

        let mut adapter = MemberKeyEmitter { inner: self };
        value.serialize_members(&mut adapter)?;

        // End the object.
        self.stack.pop();
        self.output.push('}');
        self.after_value();
        let _ = schema; // schema unused for top-level walk; useful for codecs that emit type tags
        Ok(())
    }

    fn write_list(
        &mut self,
        _schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.before_value();
        self.output.push('[');
        self.stack.push(Frame::Array { first: true });
        write_elements(self)?;
        self.stack.pop();
        self.output.push(']');
        self.after_value();
        Ok(())
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.before_value();
        self.output.push('{');
        self.stack.push(Frame::Map {
            first: true,
            expecting_key: true,
        });
        write_entries(self)?;
        self.stack.pop();
        self.output.push('}');
        self.after_value();
        Ok(())
    }

    fn write_boolean(&mut self, _schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).boolean(value);
        self.after_value();
        Ok(())
    }

    fn write_byte(&mut self, _schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::NegInt(value as i64));
        self.after_value();
        Ok(())
    }

    fn write_short(&mut self, _schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::NegInt(value as i64));
        self.after_value();
        Ok(())
    }

    fn write_integer(&mut self, _schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::NegInt(value as i64));
        self.after_value();
        Ok(())
    }

    fn write_long(&mut self, _schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::NegInt(value));
        self.after_value();
        Ok(())
    }

    fn write_float(&mut self, _schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::Float(value as f64));
        self.after_value();
        Ok(())
    }

    fn write_double(&mut self, _schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).number(Number::Float(value));
        self.after_value();
        Ok(())
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.before_value();
        // BigInteger is rendered as a raw JSON number.
        JsonValueWriter::new(&mut self.output).write_raw_value(value.as_ref());
        self.after_value();
        Ok(())
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).write_raw_value(value.as_ref());
        self.after_value();
        Ok(())
    }

    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).string(value);
        self.after_value();
        Ok(())
    }

    fn write_blob(&mut self, _schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.before_value();
        let encoded = base64::encode(value.as_ref());
        JsonValueWriter::new(&mut self.output).string(&encoded);
        self.after_value();
        Ok(())
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.before_value();
        let format = timestamp_format(schema.timestamp_format().map(|t| t.format()));
        JsonValueWriter::new(&mut self.output)
            .date_time(value, format)
            .map_err(|e| SerdeError::custom(format!("invalid timestamp: {e}")))?;
        self.after_value();
        Ok(())
    }

    fn write_document(&mut self, _schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).document(value);
        self.after_value();
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        self.before_value();
        JsonValueWriter::new(&mut self.output).null();
        self.after_value();
        Ok(())
    }
}

// =================================================================================================
// Deserializer
// =================================================================================================

/// JSON [`ShapeDeserializer`] backed by `aws-smithy-json`.
///
/// Holds a [`JsonTokenIterator`] wrapped in a [`std::cell::RefCell`].
/// Interior mutability is needed because [`ShapeDeserializer::is_null`]
/// takes `&self` but conceptually peeks at the next token.
pub struct JsonShapeDeserializer<'a> {
    tokens: std::cell::RefCell<Peekable<JsonTokenIterator<'a>>>,
    /// Whether to honor `@jsonName` on members during deserialization.
    use_json_name: bool,
    /// How many nested shapes deep the walk currently sits. An
    /// `Rc<Cell<usize>>` so the depth guard can own a handle to the counter
    /// without borrowing the deserializer, which the recursive readers hand to
    /// a consumer by mutable reference while the guard is still alive.
    depth: std::rc::Rc<std::cell::Cell<usize>>,
}

/// The deepest nesting a JSON request may reach before it is refused.
///
/// The struct/list/map readers recurse into one another and `read_document`
/// recurses into itself, so request-chosen nesting drives the recursion depth
/// directly. Without a bound a payload of ten thousand open brackets would
/// recurse until the thread stack is exhausted and the process aborts.
/// Rejecting past a fixed depth turns that into a clean deserialization error.
const MAX_DEPTH: usize = 127;

/// Increments the deserializer's depth on entry to a recursive reader and
/// restores it on drop, so an early return cannot leave the count too high.
struct DepthGuard(std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

impl<'a> JsonShapeDeserializer<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self::new_with_config(input, false)
    }

    pub fn new_with_config(input: &'a [u8], use_json_name: bool) -> Self {
        Self {
            tokens: std::cell::RefCell::new(json_token_iter(input).peekable()),
            use_json_name,
            depth: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    }

    /// Enter one level of nesting, refusing to descend past [`MAX_DEPTH`].
    ///
    /// The returned guard restores the previous depth when it drops, so a reader
    /// that returns early still leaves the count correct. Called at the entry of
    /// each recursive reader to bound stack use on request-controlled nesting.
    fn enter_depth(&self) -> Result<DepthGuard, SerdeError> {
        let next = self.depth.get() + 1;
        if next > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "input nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        self.depth.set(next);
        Ok(DepthGuard(std::rc::Rc::clone(&self.depth)))
    }

    fn next_token(&mut self) -> Result<Token<'a>, SerdeError> {
        match self.tokens.get_mut().next() {
            Some(Ok(t)) => Ok(t),
            Some(Err(e)) => Err(json_err(e)),
            None => Err(SerdeError::custom("unexpected end of JSON input")),
        }
    }

    /// Returns the kind of the next token without consuming it.
    fn peek_kind(&mut self) -> Result<TokenKind, SerdeError> {
        let mut tokens = self.tokens.borrow_mut();
        match tokens.peek() {
            Some(Ok(t)) => Ok(TokenKind::from(t)),
            Some(Err(_)) => {
                // Surface the error by consuming the token (which clones it
                // out via next()) on the *next* mutable call.
                Err(SerdeError::custom("json parse error in lookahead"))
            }
            None => Err(SerdeError::custom("unexpected end of JSON input")),
        }
    }

    fn skip_value(&mut self) -> Result<(), SerdeError> {
        let mut depth: usize = 0;
        loop {
            let tok = self.next_token()?;
            match tok {
                Token::StartArray { .. } | Token::StartObject { .. } => {
                    depth += 1;
                    // An unknown member's value is skipped structurally rather
                    // than deserialized, so bound its nesting the same way the
                    // typed readers bound theirs: refuse past MAX_DEPTH instead
                    // of looping over request-chosen nesting without limit.
                    if depth > MAX_DEPTH {
                        return Err(SerdeError::custom(format!(
                            "skipped value nests deeper than the limit of {MAX_DEPTH}"
                        )));
                    }
                }
                Token::EndArray { .. } | Token::EndObject { .. } => {
                    if depth == 0 {
                        return Err(SerdeError::custom(
                            "unexpected closing token while skipping value",
                        ));
                    }
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Token::ObjectKey { .. } => continue,
                Token::ValueBool { .. }
                | Token::ValueNull { .. }
                | Token::ValueNumber { .. }
                | Token::ValueString { .. } => {
                    if depth == 0 {
                        return Ok(());
                    }
                }
            }
        }
    }
}

fn json_err(e: DeserializeError) -> SerdeError {
    SerdeError::custom(format!("json parse error: {e}"))
}

/// The shape of a JSON token, without the embedded value or borrow. Used by
/// [`JsonShapeDeserializer::peek_kind`] when we need lookahead through `&self`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum TokenKind {
    StartArray,
    EndArray,
    ObjectKey,
    StartObject,
    EndObject,
    ValueBool,
    ValueNull,
    ValueNumber,
    ValueString,
}

impl From<&Token<'_>> for TokenKind {
    fn from(t: &Token<'_>) -> Self {
        match t {
            Token::StartArray { .. } => Self::StartArray,
            Token::EndArray { .. } => Self::EndArray,
            Token::ObjectKey { .. } => Self::ObjectKey,
            Token::StartObject { .. } => Self::StartObject,
            Token::EndObject { .. } => Self::EndObject,
            Token::ValueBool { .. } => Self::ValueBool,
            Token::ValueNull { .. } => Self::ValueNull,
            Token::ValueNumber { .. } => Self::ValueNumber,
            Token::ValueString { .. } => Self::ValueString,
        }
    }
}

/// The JSON grammar term for a token, in the vocabulary a CALLER uses.
///
/// Every `SerdeError` a codec raises can reach the client verbatim: the dispatch
/// generated by `smithy-macros` interpolates `SerdeError::to_string()` straight
/// into the `ValidationException` body. So a `{t:?}` here does not stay internal
/// — it puts the Rust token struct, its field names and a byte offset into the
/// request body on the wire (`expected string, got ValueNull { offset:
/// Offset(66) }`). This impl is what the "expected X, got Y" messages format
/// instead, so `Y` names a JSON type the caller can act on.
impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::StartArray => "array start",
            Self::EndArray => "array end",
            Self::ObjectKey => "object key",
            Self::StartObject => "object start",
            Self::EndObject => "object end",
            Self::ValueBool => "boolean",
            Self::ValueNull => "null",
            Self::ValueNumber => "number",
            Self::ValueString => "string",
        })
    }
}

fn unescape(s: EscapedStr<'_>) -> Result<String, SerdeError> {
    s.to_unescaped()
        .map(|c| c.into_owned())
        .map_err(|e| SerdeError::custom(format!("invalid string escape: {e}")))
}

fn parse_str_to_number<T: FromStr>(s: &str, ty: &str) -> Result<T, SerdeError>
where
    T::Err: std::fmt::Display,
{
    T::from_str(s).map_err(|e| SerdeError::custom(format!("invalid {ty}: {e}")))
}

fn number_to_i64(n: Number) -> Result<i64, SerdeError> {
    match n {
        Number::PosInt(v) => i64::try_from(v)
            .map_err(|_| SerdeError::custom(format!("number {v} does not fit in i64"))),
        Number::NegInt(v) => Ok(v),
        Number::Float(v) => {
            if v.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&v) {
                Ok(v as i64)
            } else {
                Err(SerdeError::custom(format!(
                    "float {v} cannot be represented as i64"
                )))
            }
        }
    }
}

fn number_to_f64(n: Number) -> f64 {
    match n {
        Number::PosInt(v) => v as f64,
        Number::NegInt(v) => v as f64,
        Number::Float(v) => v,
    }
}

impl<'a> ShapeDeserializer for JsonShapeDeserializer<'a> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let _depth = self.enter_depth()?;
        // Allow `null` as an absent struct.
        if self.peek_kind()? == TokenKind::ValueNull {
            self.next_token()?;
            return Ok(());
        }
        match self.next_token()? {
            Token::StartObject { .. } => {}
            t => {
                return Err(SerdeError::custom(format!(
                    "expected object start, got {}",
                    TokenKind::from(&t)
                )))
            }
        }
        loop {
            match self.next_token()? {
                Token::EndObject { .. } => return Ok(()),
                Token::ObjectKey { key, .. } => {
                    let key = unescape(key)?;
                    // Look up the member by wire name.
                    let member = if self.use_json_name {
                        schema.members().iter().copied().find(|m| {
                            m.json_name().map(|jn| jn.value()).unwrap_or_else(|| {
                                m.member_name()
                                    .expect("struct member schema must have a member name")
                            }) == key.as_str()
                        })
                    } else {
                        schema
                            .members()
                            .iter()
                            .copied()
                            .find(|m| m.member_name() == Some(key.as_str()))
                    };
                    if let Some(member) = member {
                        consumer(member, self)?;
                    } else {
                        // Unknown member: skip its value to keep the parser
                        // in sync with the input. This matches AWS's
                        // forward-compatible behaviour.
                        self.skip_value()?;
                    }
                }
                t => {
                    return Err(SerdeError::custom(format!(
                        "expected object key or end, got {}",
                        TokenKind::from(&t)
                    )))
                }
            }
        }
    }

    fn read_list(
        &mut self,
        _schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let _depth = self.enter_depth()?;
        if self.peek_kind()? == TokenKind::ValueNull {
            self.next_token()?;
            return Ok(());
        }
        match self.next_token()? {
            Token::StartArray { .. } => {}
            t => {
                return Err(SerdeError::custom(format!(
                    "expected array start, got {}",
                    TokenKind::from(&t)
                )))
            }
        }
        loop {
            if self.peek_kind()? == TokenKind::EndArray {
                self.next_token()?;
                return Ok(());
            }
            consumer(self)?;
        }
    }

    fn read_map(
        &mut self,
        _schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let _depth = self.enter_depth()?;
        if self.peek_kind()? == TokenKind::ValueNull {
            self.next_token()?;
            return Ok(());
        }
        match self.next_token()? {
            Token::StartObject { .. } => {}
            t => {
                return Err(SerdeError::custom(format!(
                    "expected object start, got {}",
                    TokenKind::from(&t)
                )))
            }
        }
        loop {
            match self.next_token()? {
                Token::EndObject { .. } => return Ok(()),
                Token::ObjectKey { key, .. } => {
                    let key = unescape(key)?;
                    consumer(key, self)?;
                }
                t => {
                    return Err(SerdeError::custom(format!(
                        "expected object key or end, got {}",
                        TokenKind::from(&t)
                    )))
                }
            }
        }
    }

    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        match self.next_token()? {
            Token::ValueBool { value, .. } => Ok(value),
            t => Err(SerdeError::custom(format!(
                "expected boolean, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        let v = self.read_long(schema)?;
        i8::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i8")))
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        let v = self.read_long(schema)?;
        i16::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i16")))
    }

    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        let v = self.read_long(schema)?;
        i32::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i32")))
    }

    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        match self.next_token()? {
            Token::ValueNumber { value, .. } => number_to_i64(value),
            t => Err(SerdeError::custom(format!(
                "expected number, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        Ok(self.read_double(schema)? as f32)
    }

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        // awsJson carries the special float values (`NaN`, `Infinity`,
        // `-Infinity`) as strings. `aws-smithy-json` already implements that
        // rule (and rejects non-finite numeric literals), so delegate to it
        // rather than re-parsing the string here.
        let token = self.tokens.get_mut().next();
        match expect_number_or_null(token).map_err(json_err)? {
            Some(value) => Ok(number_to_f64(value)),
            None => Err(SerdeError::custom("expected number, got null")),
        }
    }

    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        match self.next_token()? {
            Token::ValueNumber { value, .. } => match value {
                Number::PosInt(v) => parse_str_to_number(&v.to_string(), "BigInteger"),
                Number::NegInt(v) => parse_str_to_number(&v.to_string(), "BigInteger"),
                Number::Float(_) => Err(SerdeError::custom(
                    "BigInteger cannot be represented as a JSON float",
                )),
            },
            Token::ValueString { value, .. } => {
                let s = unescape(value)?;
                parse_str_to_number(&s, "BigInteger")
            }
            t => Err(SerdeError::custom(format!(
                "expected number or string for BigInteger, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        match self.next_token()? {
            Token::ValueNumber { value, .. } => match value {
                Number::PosInt(v) => parse_str_to_number(&v.to_string(), "BigDecimal"),
                Number::NegInt(v) => parse_str_to_number(&v.to_string(), "BigDecimal"),
                Number::Float(v) => parse_str_to_number(&v.to_string(), "BigDecimal"),
            },
            Token::ValueString { value, .. } => {
                let s = unescape(value)?;
                parse_str_to_number(&s, "BigDecimal")
            }
            t => Err(SerdeError::custom(format!(
                "expected number or string for BigDecimal, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        match self.next_token()? {
            Token::ValueString { value, .. } => unescape(value),
            t => Err(SerdeError::custom(format!(
                "expected string, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        match self.next_token()? {
            Token::ValueString { value, .. } => {
                let s = unescape(value)?;
                let bytes = base64::decode(&s)
                    .map_err(|e| SerdeError::custom(format!("invalid base64: {e}")))?;
                Ok(Blob::new(bytes))
            }
            t => Err(SerdeError::custom(format!(
                "expected base64 string, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        let format = timestamp_format(schema.timestamp_format().map(|t| t.format()));
        match self.next_token()? {
            Token::ValueNumber { value, .. } => {
                let secs = number_to_f64(value);
                Ok(DateTime::from_secs_f64(secs))
            }
            Token::ValueString { value, .. } => {
                let s = unescape(value)?;
                DateTime::from_str(&s, format)
                    .map_err(|e| SerdeError::custom(format!("invalid timestamp: {e}")))
            }
            t => Err(SerdeError::custom(format!(
                "expected timestamp, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        // Recursive descent: read whatever's next and turn it into a Document.
        fn read_doc(d: &mut JsonShapeDeserializer<'_>) -> Result<Document, SerdeError> {
            let _depth = d.enter_depth()?;
            match d.next_token()? {
                Token::ValueNull { .. } => Ok(Document::Null),
                Token::ValueBool { value, .. } => Ok(Document::Bool(value)),
                Token::ValueNumber { value, .. } => Ok(Document::Number(value)),
                Token::ValueString { value, .. } => Ok(Document::String(unescape(value)?)),
                Token::StartArray { .. } => {
                    let mut out = Vec::new();
                    loop {
                        if d.peek_kind()? == TokenKind::EndArray {
                            d.next_token()?;
                            return Ok(Document::Array(out));
                        }
                        out.push(read_doc(d)?);
                    }
                }
                Token::StartObject { .. } => {
                    let mut out = std::collections::HashMap::new();
                    loop {
                        match d.next_token()? {
                            Token::EndObject { .. } => return Ok(Document::Object(out)),
                            Token::ObjectKey { key, .. } => {
                                let k = unescape(key)?;
                                let v = read_doc(d)?;
                                out.insert(k, v);
                            }
                            t => {
                                return Err(SerdeError::custom(format!(
                                    "unexpected token in document object: {}",
                                    TokenKind::from(&t)
                                )))
                            }
                        }
                    }
                }
                t => Err(SerdeError::custom(format!(
                    "unexpected token in document: {}",
                    TokenKind::from(&t)
                ))),
            }
        }
        read_doc(self)
    }

    fn is_null(&self) -> bool {
        // Peek at the next token; treat parse errors / EOF as "not null".
        let mut tokens = self.tokens.borrow_mut();
        matches!(tokens.peek(), Some(Ok(Token::ValueNull { .. })))
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        match self.next_token()? {
            Token::ValueNull { .. } => Ok(()),
            t => Err(SerdeError::custom(format!(
                "expected null, got {}",
                TokenKind::from(&t)
            ))),
        }
    }

    fn container_size(&self) -> Option<usize> {
        // JSON tokens don't carry a length.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_schema::{ShapeId, ShapeType};

    static DOC_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("com.test#Doc", "com.test", "Doc"),
        ShapeType::Structure,
        &[],
    );

    /// A JSON document nested far past the depth bound is refused with a clean
    /// error instead of recursing until the stack is exhausted.
    #[test]
    fn deeply_nested_json_document_is_rejected() {
        let depth = MAX_DEPTH + 50;
        let mut body = String::new();
        for _ in 0..depth {
            body.push('[');
        }
        for _ in 0..depth {
            body.push(']');
        }
        let mut d = JsonShapeDeserializer::new(body.as_bytes());
        let err = d
            .read_document(&DOC_SCHEMA)
            .expect_err("a document nested past the depth bound must be refused");
        assert!(
            format!("{err}").contains("nests deeper than the limit"),
            "unexpected error: {err}"
        );
    }

    /// A document within the bound still parses, so the guard rejects only
    /// genuinely excessive nesting.
    #[test]
    fn moderately_nested_json_document_is_accepted() {
        let depth = 10;
        let mut body = String::new();
        for _ in 0..depth {
            body.push('[');
        }
        for _ in 0..depth {
            body.push(']');
        }
        let mut d = JsonShapeDeserializer::new(body.as_bytes());
        d.read_document(&DOC_SCHEMA)
            .expect("nesting within the bound must still parse");
    }

    /// The guard restores depth as it unwinds, so sibling branches do not
    /// accumulate depth from one another: a wide-but-shallow document parses.
    #[test]
    fn depth_is_restored_between_sibling_branches() {
        // Many sibling arrays, each shallow. If the guard failed to restore
        // depth on the way out, the running count would climb with each sibling
        // and trip the bound even though no single branch is deep.
        let mut body = String::from("[");
        for i in 0..(MAX_DEPTH + 50) {
            if i > 0 {
                body.push(',');
            }
            body.push_str("[1]");
        }
        body.push(']');
        let mut d = JsonShapeDeserializer::new(body.as_bytes());
        d.read_document(&DOC_SCHEMA)
            .expect("shallow siblings must not accumulate depth");
    }
}
