// SPDX-License-Identifier: CC0-1.0

//! How this service writes a `double` on the wire: with a fractional part, always.
//!
//! CloudFormation's own XML envelope renders a floating-point member as a DOUBLE, so an
//! integral one carries its fractional part — a `PercentageCompleted` of a hundred per cent
//! is `100.0`, never `100`. Rust's shortest decimal rendering drops that part for an
//! integral value, which is the whole of the difference: an SDK that round-trips the member
//! then reads a different literal from the one the service wrote, and a client or test
//! matching the body does not match.
//!
//! The rule is about the member's modelled TYPE, not about one operation: it holds for every
//! `double` and `float` the model declares, wherever one appears — a member of the output
//! struct, a member of a struct inside a list, a map value. So it is applied by the
//! SERIALIZER this service answers modelled requests through
//! ([`smithy_server_runtime::Service::override_protocol`]) rather than by any handler, which
//! cannot reach the rendering of its own output, and it is driven by the member schema the
//! generated code already hands the serializer rather than by the element's name.
//!
//! [`DoublesWithAFractionalPart`] is that serializer: every write delegates to the one the
//! framework's protocol built, unchanged, except a floating-point write — which is rendered
//! here and handed on as the TEXT of the very same member, so the element name, its
//! position and every other byte of the envelope stay the framework's. Descending into a
//! struct, a list or a map re-wraps, so the rule reaches a double at any depth.

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// A floating-point value as CloudFormation's envelope writes it: the shortest decimal
/// rendering that round-trips, with a fractional part added when that rendering has none.
///
/// A rendering already carrying a `.` or an exponent is a double to a reader and is left
/// exactly as it is, as is a value that is not a finite number and so has no decimal form at
/// all.
fn rendered_as_a_double(value: f64) -> String {
    let rendered = value.to_string();
    if !value.is_finite() || rendered.contains(['.', 'e', 'E']) {
        return rendered;
    }
    format!("{rendered}.0")
}

/// One struct, serialized through [`DoublesWithAFractionalPart`].
///
/// Needed because a nested struct is written by handing the serializer the struct VALUE: the
/// framework's serializer then drives that value's own members with ITSELF, so the wrapper
/// has to be re-applied on the way in rather than relying on the outer call.
struct StructWithDoubleMembers<'a> {
    inner: &'a dyn SerializableStruct,
}

impl SerializableStruct for StructWithDoubleMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.inner
            .serialize_members(&mut DoublesWithAFractionalPart { inner: s })
    }
}

/// The serializer this service's responses are written through: the framework's, with every
/// floating-point member rendered the way [`rendered_as_a_double`] states.
pub struct DoublesWithAFractionalPart<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl<'a> DoublesWithAFractionalPart<'a> {
    /// Wrap the serializer the framework's protocol is about to write an output or an error
    /// through.
    pub fn new(inner: &'a mut dyn ShapeSerializer) -> Self {
        Self { inner }
    }
}

impl ShapeSerializer for DoublesWithAFractionalPart<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_struct(schema, &StructWithDoubleMembers { inner: value })
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|elements| {
            write_elements(&mut DoublesWithAFractionalPart { inner: elements })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|entries| {
            write_entries(&mut DoublesWithAFractionalPart { inner: entries })
        })
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

    /// A `float` is rendered by the same rule a `double` is: both are floating-point members,
    /// and the envelope writes both with a fractional part.
    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner
            .write_string(schema, &rendered_as_a_double(f64::from(value)))
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        // Handed on as the TEXT of this same member, so the element the framework would have
        // written is the element it writes — only its characters differ.
        self.inner
            .write_string(schema, &rendered_as_a_double(value))
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

/// One serializable value, written through [`DoublesWithAFractionalPart`].
///
/// This is what the protocol hands the framework's `serialize_output` / `serialize_error` in
/// place of the value it was given, so the whole envelope — status, element names, namespace,
/// request id — stays the framework's and only the floating-point members are re-rendered.
pub struct WithDoublesAsDoubles<'a> {
    inner: &'a dyn SerializableStruct,
}

impl<'a> WithDoublesAsDoubles<'a> {
    pub fn new(inner: &'a dyn SerializableStruct) -> Self {
        Self { inner }
    }
}

impl SerializableStruct for WithDoublesAsDoubles<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.inner
            .serialize_members(&mut DoublesWithAFractionalPart::new(s))
    }
}

#[cfg(test)]
mod tests {
    use super::rendered_as_a_double;

    /// A double is written with a fractional part whatever its magnitude: an integral value
    /// gains one, a value that already has one keeps exactly the digits that round-trip, and
    /// a value with no decimal form at all is left as it is.
    #[test]
    fn a_double_is_written_with_a_fractional_part() {
        for (value, expected) in [
            (100.0_f64, "100.0"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-7.0, "-7.0"),
            (33.5, "33.5"),
            (0.125, "0.125"),
            (1e22, "10000000000000000000000.0"),
            (f64::NAN, "NaN"),
        ] {
            let rendered = rendered_as_a_double(value);
            assert_eq!(
                rendered, expected,
                "case {value:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
