// SPDX-License-Identifier: CC0-1.0

//! How this service puts a SUCCESS response on the wire: as a COMPLETE document.
//!
//! The IoT Jobs device API answers every 2xx with a JSON object that carries EVERY
//! member its operation's output shape models, and reports a member it has no value
//! for as JSON `null` rather than leaving the key out. The generated serializer does
//! the opposite: it skips an `Option` member holding `None`, so an operation with
//! nothing to report serialises an empty document (`{}`) and a client reading the
//! response finds no member at all — a success with no payload.
//!
//! The recorded corpus `simparity-wire-aws-v1` is the evidence for the rule at the
//! response-document level:
//!
//! * `PUT /things/{thingName}/jobs/$next` against a thing with nothing pending is a
//!   200 whose body is `{"execution": null}`;
//! * `POST /things/{thingName}/jobs/{jobId}` that asks for neither optional member is
//!   a 200 whose body is `{"executionState": null, "jobDocument": null}`;
//! * a queued summary in `GET /things/{thingName}/jobs` carries `"startedAt": null`
//!   for an execution that has not started.
//!
//! [`ResponseDocument`] is what every operation of this service answers through, so
//! the rule is stated once and cannot hold for one operation and not another. It is
//! generic over the output type and takes the member list from the operation's OWN
//! output [`Schema`] (`Operation::OUTPUT_SCHEMA`), so a member added to the model
//! reaches the wire without touching this file, and nothing here names an operation.
//!
//! Nested structures are written exactly as the handler's projection built them (see
//! `crate::state::to_job_execution` / `to_execution_state`, whose two deliberate
//! omissions are pinned by recorded steps of the same corpus); this type completes the
//! response DOCUMENT, which is where an omission is observable as an empty body.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// An operation output as this service writes it: every member of `shape` reaches the
/// wire, the ones the output has no value for as JSON `null`.
///
/// Constructed in a handler's `handle` body with the operation's own output schema:
///
/// ```ignore
///     Ok(ResponseDocument::new(
///         SomeOperationOutput { member: None },
///         Self::OUTPUT_SCHEMA,
///     ))
/// ```
pub struct ResponseDocument<T> {
    output: T,
    shape: &'static Schema,
}

impl<T> ResponseDocument<T> {
    /// Wrap `output`, completing it against `shape` — the operation's
    /// `OUTPUT_SCHEMA`, so the member list is the model's and not a copy of it.
    pub fn new(output: T, shape: &'static Schema) -> Self {
        Self { output, shape }
    }

    /// The wrapped output, for a caller that needs to read back what it built.
    pub fn output(&self) -> &T {
        &self.output
    }
}

impl<T: SerializableStruct> SerializableStruct for ResponseDocument<T> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let mut completing = Complete {
            inner: serializer,
            shape: self.shape,
            written: Vec::new(),
        };
        self.output.serialize_members(&mut completing)?;
        completing.fill_absent()
    }
}

/// A [`ShapeSerializer`] that notes which members the output wrote and passes every
/// write straight through, so that [`Complete::fill_absent`] can write JSON `null` for
/// the members that never reached it.
///
/// It wraps the serializer the framework hands to `serialize_members`, so a member the
/// output DOES write is written exactly as the model says — including an `@httpHeader`
/// or `@httpResponseCode` binding, which the framework's own binder below this
/// serializer still routes.
struct Complete<'s> {
    inner: &'s mut dyn ShapeSerializer,
    shape: &'static Schema,
    /// The `member_index` of every member the output wrote — the identity the
    /// generated schema gives a member, so noting one allocates nothing.
    written: Vec<usize>,
}

impl Complete<'_> {
    /// Note that the output wrote `member`, whatever it wrote for it.
    fn written_member(&mut self, member: &Schema) {
        if let Some(index) = member.member_index() {
            self.written.push(index);
        }
    }

    /// Write JSON `null` for every member of the output shape the output wrote
    /// nothing for.
    fn fill_absent(&mut self) -> ::std::result::Result<(), SerdeError> {
        for member in self.shape.members() {
            let Some(index) = member.member_index() else {
                continue;
            };
            if !self.written.contains(&index) {
                self.inner.write_null(member)?;
            }
        }
        Ok(())
    }
}

/// Emit the [`ShapeSerializer`] methods that write a struct member: the write the
/// output asked for, plus a note that the member reached the wire.
///
/// Every member write is routed through the same note — a member is a member whatever
/// its type — so the completion cannot be right for a string and wrong for a list.
macro_rules! note_and_pass_through {
    ($( fn $method:ident($($arg:ident: $ty:ty),*); )*) => {
        $(
            fn $method(
                &mut self,
                schema: &Schema,
                $($arg: $ty),*
            ) -> ::std::result::Result<(), SerdeError> {
                self.written_member(schema);
                self.inner.$method(schema, $($arg),*)
            }
        )*
    };
}

impl ShapeSerializer for Complete<'_> {
    note_and_pass_through! {
        fn write_list(
            write_elements: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
        fn write_map(
            write_entries: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
        fn write_struct(value: &dyn SerializableStruct);
        fn write_boolean(value: bool);
        fn write_byte(value: i8);
        fn write_short(value: i16);
        fn write_integer(value: i32);
        fn write_long(value: i64);
        fn write_float(value: f32);
        fn write_double(value: f64);
        fn write_big_integer(value: &BigInteger);
        fn write_big_decimal(value: &BigDecimal);
        fn write_string(value: &str);
        fn write_blob(value: &[u8]);
        fn write_timestamp(value: &DateTime);
        fn write_document(value: &Document);
        fn write_null();
    }
}
