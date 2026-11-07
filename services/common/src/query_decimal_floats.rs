// SPDX-License-Identifier: CC0-1.0

//! A FLOATING-POINT member rendered on the XML wire the way AWS renders one: always
//! carrying a fractional part.
//!
//! # The gap this closes
//!
//! AWS renders a Smithy `double` (and a `float`) in an XML response as a decimal with a
//! fractional part — a whole value reads `0.0`, `128.0`, `256.0`, never `0`, `128`, `256`
//! — while Rust's own rendering drops the fractional part of a whole value
//! (`0.0_f64.to_string() == "0"`). The framework's XML writer hands the value straight to
//! `to_string()`
//! (`vendor/smithy-schema-server/src/xml.rs`'s `write_double`/`write_float`), so every
//! whole-valued floating-point member of every response leaves the emulator in a form
//! no AWS service answers with: a client reading the text of such an element — a capacity
//! limit, a price, a ratio — sees an integer where the API documents a decimal.
//!
//! The structural fix belongs in that writer, which a service crate cannot edit
//! (`vendor/` is read-only), so this module closes it on the SERVICE side, at the typed
//! [`Operation`] seam: the wrapped operation's output is serialized with every
//! floating-point member written as the decimal TEXT of its value
//! ([`decimal_double_text`]), and nothing else about the walk changes — the element's
//! name, its order and its schema stay the ones the model gave it, and an INTEGER member
//! is untouched (`<MinCapacity>2</MinCapacity>` of an integer member stays `2`, which is
//! what AWS answers for one).
//!
//! The one-shape version of this rewrite already exists in a sibling service, written out
//! by hand for the two doubles that one operation answers with
//! (`services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/types/describe_storage_types.rs`).
//! This module is the same rule as a WRAPPER, so a service with doubles in dozens of
//! shapes does not hand-author a serializer per shape: the rule is the protocol's, not any
//! one shape's.
//!
//! # How a service serves its operations through it
//!
//! Exactly as it serves them through any other wrapper of this crate — from its routing
//! table, where its operations are constructed:
//!
//! ```ignore
//! table.serve_behind(
//!     crate::ops::describe_db_engine_versions::DescribeDBEngineVersions::new(engine),
//!     CallKind::Reporting,
//!     DecimalFloatsOperation::new,
//! );
//! ```
//!
//! The wrapper runs the framework's own deserialize -> validate -> handle -> serialize
//! sequence ([`smithy_server_runtime::run_operation_http`]) and forwards the wrapped
//! operation's modelled auth schemes, so no status, code or message is this module's.
//!
//! A service that renders its own protocol envelope instead of serving each operation
//! behind a wrapper reaches the same rule at that one seam, where its output is a
//! `&dyn SerializableStruct`: [`DecimalFloatsRef`].

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, PreDispatchProtocol, Service,
};

/// The decimal text AWS renders a Smithy `double` as.
///
/// A whole value grows one fractional digit (`0` -> `0.0`, `256` -> `256.0`) and a value
/// that already has fractional digits keeps the ones it has (`0.5`, `2048.25`). A
/// non-finite double has no decimal form at all, so it falls back to Rust's own rendering
/// rather than being given a fake one.
#[must_use]
pub fn decimal_double_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// The same rendering for a Smithy `float`, the other floating-point type: the wire form
/// of the two is the same, and a `float` is formatted from its own value rather than from
/// a widened one, so no binary32 value picks up the digits of its binary64 neighbour.
#[must_use]
pub fn decimal_float_text(value: f32) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// One output shape, serialized with every floating-point member rendered as decimal
/// text.
///
/// Built by [`DecimalFloatsOperation`]; there is no reason to name this type elsewhere.
pub struct DecimalFloats<T>(pub T);

impl<T: SerializableStruct> SerializableStruct for DecimalFloats<T> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DecimalFloatSerializer { inner: s })
    }
}

/// One BORROWED structure serialized with the same rule.
///
/// Two callers need it. A structure NESTED under a wrapped output is re-wrapped so the
/// rule reaches every depth: the member that carries a double is rarely the output's own
/// (it sits inside a reported resource, inside a list of them), and the framework's codec
/// walks a nested structure with its OWN serializer, so the wrapper has to re-enter at
/// each level. And a service that renders its whole protocol envelope itself — rather
/// than serving each operation behind [`DecimalFloatsOperation`] — reaches its output as a
/// `&dyn SerializableStruct` and wraps it here.
pub struct DecimalFloatsRef<'a>(pub &'a dyn SerializableStruct);

impl SerializableStruct for DecimalFloatsRef<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.0.serialize_members(&mut DecimalFloatSerializer { inner: s })
    }
}

/// The serializer the wrapped output is walked with: every call is forwarded to the
/// codec's own serializer unchanged, except a floating-point leaf, which is written as
/// its decimal text.
struct DecimalFloatSerializer<'a> {
    inner: &'a mut dyn ShapeSerializer,
}

impl ShapeSerializer for DecimalFloatSerializer<'_> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner.write_struct(schema, &DecimalFloatsRef(value))
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, &|inner| {
            write_elements(&mut DecimalFloatSerializer { inner })
        })
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, &|inner| {
            write_entries(&mut DecimalFloatSerializer { inner })
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

    /// The one rewrite on the `float` side: the member's own schema, carrying the text of
    /// its value.
    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner.write_string(schema, &decimal_float_text(value))
    }

    /// The one rewrite on the `double` side.
    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner.write_string(schema, &decimal_double_text(value))
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

/// One modelled operation served with every floating-point member of its response
/// rendered as decimal text.
pub struct DecimalFloatsOperation<O> {
    /// The operation that serves the call.
    operation: O,
    /// The protocol envelope the framework renders a pre-dispatch failure of this
    /// operation in — the same value the generated shim passes.
    pre_dispatch: PreDispatchProtocol<'static>,
}

impl<O> DecimalFloatsOperation<O> {
    /// Wrap one operation, already constructed from the shared engine, whose pre-dispatch
    /// failures render in `pre_dispatch`.
    pub fn new(operation: O, pre_dispatch: PreDispatchProtocol<'static>) -> Self {
        Self {
            operation,
            pre_dispatch,
        }
    }
}

impl<O: Operation> Operation for DecimalFloatsOperation<O> {
    type Input = O::Input;
    type Output = DecimalFloats<O::Output>;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The handler's own answer, untouched: only how it is RENDERED changes, which is
        // the wrapper around it rather than anything about the output it built.
        self.operation.handle(input, ctx).await.map(DecimalFloats)
    }
}

impl<O> ErasedOperation for DecimalFloatsOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation: the handler it calls is the wrapped one, so the only difference
            // is the serializer the output is written with.
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, self.pre_dispatch).await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: wrapping must not change which
        // credentials sign a call.
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::{decimal_double_text, decimal_float_text};

    /// A double carries a fractional part whichever value it holds: a whole value grows
    /// one, and a fractional one keeps the digits it has.
    #[test]
    fn a_double_always_renders_with_a_fractional_part() {
        // Rows differ along what the rule generalises over: whether the value is whole,
        // its sign, its magnitude, how many fractional digits it already has, and the
        // two Aurora capacity limits the member this was found on reports.
        let rows: [(f64, &str); 9] = [
            (0.0, "0.0"),
            (0.5, "0.5"),
            (1.0, "1.0"),
            (-1.0, "-1.0"),
            (-0.25, "-0.25"),
            (16.0, "16.0"),
            (128.0, "128.0"),
            (256.0, "256.0"),
            (2048.25, "2048.25"),
        ];
        for (value, expected) in rows {
            let rendered = decimal_double_text(value);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// A value with no decimal form keeps Rust's own rendering rather than being given a
    /// fractional part it does not have.
    #[test]
    fn a_non_finite_double_is_not_given_a_decimal_form() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let rendered = decimal_double_text(value);
            assert_eq!(
                rendered,
                value.to_string(),
                "case {value}: expected the plain rendering, got {rendered:?}"
            );
        }
    }

    /// The `float` side renders from the binary32 value itself: `0.1f32` reads as the
    /// digits that round-trip it, not as the binary64 expansion of the same bits.
    #[test]
    fn a_float_renders_from_its_own_value() {
        let rows: [(f32, &str); 4] = [(0.0, "0.0"), (2.0, "2.0"), (0.1, "0.1"), (-0.5, "-0.5")];
        for (value, expected) in rows {
            let rendered = decimal_float_text(value);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
