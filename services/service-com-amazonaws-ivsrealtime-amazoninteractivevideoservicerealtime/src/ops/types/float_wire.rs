// SPDX-License-Identifier: CC0-1.0

//! A `float` member of a response, rendered from the binary32 value it holds.
//!
//! WHY THIS MODULE EXISTS. A Smithy `float` is a binary32 value, and the decimal a client
//! sent is the decimal it reads back: a frame rate configured as `29.97` is reported as
//! `29.97`. The vendored JSON writer renders one by WIDENING it
//! (`vendor/smithy-schema-server/src/json.rs::write_float` hands
//! `Number::Float(value as f64)` to the JSON value writer), and the shortest decimal that
//! round-trips a binary64 is not the shortest decimal that round-trips the binary32 it was
//! widened from — so every fractional `float` of every response leaves the emulator as the
//! binary64 expansion of its own bits (`29.969999313354492` for the binary32 nearest
//! `29.97`). Fractional values are the normal case for a broadcast frame rate (`29.97`,
//! `59.94`), and a client that reads one back, changes an unrelated member and writes it
//! again no longer sends the value it configured.
//!
//! The structural fix belongs in that writer, which is read-only from inside the emulator,
//! so this module closes it on the SERVICE side, at the typed output seam: an operation
//! whose `Operation::Output` is [`ReportedFloats<O>`] is serialized with every `float`
//! leaf written as the decimal TEXT of its own binary32 value, as a JSON NUMBER, and
//! nothing else about the walk changes — each member keeps the schema, the name and the
//! order the model gave it, and an integer member is untouched.
//!
//! Only the OUTPUT side is rewritten. The `@range` the model declares on a `float` member
//! is enforced by the framework's constraint walk over the built INPUT, which reads the
//! numeric value off `write_float`
//! (`vendor/smithy-schema-server/src/constraints.rs`), so an input shape keeps the
//! codegen's own float leaf and every declared bound stays enforced.
//!
//! The XML side of the same rule — where the leaf is TEXT rather than a number — already
//! exists for the services that speak a protocol with one
//! (`services/common/src/query_decimal_floats.rs`). This service's sole protocol is
//! restJson1, where the member is a number, hence the decimal written here as one.
//!
//! REMOVE THIS MODULE when the vendored JSON writer renders a `float` from its own value:
//! the defect is general — every fractional `float` member of every service has it — so the
//! fix belongs there. Once it does, drop the `ReportedFloats` wrapper from the operations
//! that name it as their output and delete this file.
//! `tests/all/float_members_wire_form.rs` pins the wire form either way.

use std::str::FromStr;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// The decimal text a binary32 value is reported as.
///
/// The shortest decimal that round-trips as an `f32` (`29.97_f32` renders `29.97`, not the
/// binary64 expansion of the same bits), carrying a fractional part whichever value it
/// holds: a whole value grows one (`30` -> `30.0`), which is the form a floating-point
/// member takes on the wire, and a fractional one keeps the digits that round-trip it.
///
/// A value with no decimal form at all (a NaN, an infinity) is not given one here; see
/// [`FloatSerializer::write_float`], which leaves such a value to the framework.
#[must_use]
pub fn float_wire_text(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// One operation output, serialized with every `float` member rendered from its own
/// binary32 value.
///
/// Named as an operation's `Operation::Output`; the handler builds the modelled output it
/// always built and wraps it in this.
pub struct ReportedFloats<T>(pub T);

impl<T: SerializableStruct> SerializableStruct for ReportedFloats<T> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut FloatSerializer { inner: s })
    }
}

/// One structure NESTED under a wrapped output, re-wrapped so the rule reaches every
/// depth: the member that carries a float is rarely the output's own (it sits inside a
/// reported resource, inside a list of them), and the framework's codec walks a nested
/// structure with its OWN serializer, so the wrapper has to re-enter at each level.
struct Nested<'a>(&'a dyn SerializableStruct);

impl SerializableStruct for Nested<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut FloatSerializer { inner: s })
    }
}

/// The serializer the wrapped output is walked with: every call is forwarded to the
/// codec's own serializer unchanged, except a `float` leaf, which is written as the
/// decimal its own binary32 value spells.
struct FloatSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for FloatSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &Nested(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_list(schema, &|inner| write_elements(&mut FloatSerializer { inner }))
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_map(schema, &|inner| write_entries(&mut FloatSerializer { inner }))
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner.write_boolean(schema, value)
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner.write_byte(schema, value)
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner.write_short(schema, value)
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner.write_integer(schema, value)
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner.write_long(schema, value)
    }

    /// The one rewrite: the member's own schema, carrying the decimal its binary32 value
    /// spells, written as the arbitrary-precision decimal the codec renders as a bare
    /// JSON number rather than as a widened double.
    ///
    /// A value with no decimal text (a NaN, an infinity) is left to the framework's own
    /// rendering rather than being given a decimal form it does not have.
    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        match BigDecimal::from_str(&float_wire_text(value)) {
            Ok(decimal) => self.inner.write_big_decimal(schema, &decimal),
            Err(_) => self.inner.write_float(schema, value),
        }
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_double(schema, value)
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.inner.write_big_integer(schema, value)
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.inner.write_big_decimal(schema, value)
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_string(schema, value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner.write_blob(schema, value)
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        self.inner.write_timestamp(schema, value)
    }

    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.inner.write_document(schema, value)
    }

    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}
