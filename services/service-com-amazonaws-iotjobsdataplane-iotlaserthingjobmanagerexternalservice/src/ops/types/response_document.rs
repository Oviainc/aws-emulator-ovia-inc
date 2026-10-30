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
//! The rule holds at EVERY DEPTH, because the third recorded case above is a member of
//! a NESTED structure: a summary inside `queuedJobs`, not the response document itself.
//! A structure the output carries is therefore completed against its own shape too,
//! and so is a structure inside a list or a map. Two things make the nested walk
//! possible without re-listing any shape's members:
//!
//! * a LIST element is handed to the serializer with the ELEMENT SHAPE's own schema,
//!   which carries that shape's member list, so the completion reads it straight off
//!   the schema it is given;
//! * a structure MEMBER is handed the MEMBER schema, which carries the member's name,
//!   index and wire bindings but NOT the member list of the shape it targets, so that
//!   target is resolved by [`nested_target`] — the one place in this file that names a
//!   shape, and only because the framework's member schema does not carry it.
//!
//! The two members this service's projection does not send AT ALL stay absent rather
//! than becoming `null`: see [`unsent_members`] and `crate::state::to_job_execution`,
//! whose omissions are pinned by recorded steps of the same corpus.

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

use super::{JobExecution, JobExecutionState};

/// An operation output as this service writes it: every member of `shape` reaches the
/// wire, the ones the output has no value for as JSON `null`, and the same for every
/// structure the output carries.
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
        CompleteStruct {
            value: &self.output,
            shape: self.shape,
        }
        .serialize_members(serializer)
    }
}

/// The shape a STRUCTURE member of a completed document targets, for the members whose
/// target this file has to name: the framework's member schema carries the member's
/// own identity but not the member list of the shape it targets, and that list is what
/// the completion walks.
///
/// The lookup is keyed by the PARENT shape as well as the member name, so an entry
/// cannot mis-fire on a same-named member of another shape. A member whose target is
/// not a structure answers `None` and is written exactly as the output wrote it; a
/// structure inside a LIST needs no entry here, because the generated serializer hands
/// a list element the element shape's own schema (see this module's docs).
fn nested_target(parent: &Schema, member: &Schema) -> Option<&'static Schema> {
    match (parent.shape_id().shape_name(), member.member_name()?) {
        (
            "DescribeJobExecutionResponse" | "StartNextPendingJobExecutionResponse",
            "execution",
        ) => Some(JOB_EXECUTION_SHAPE),
        ("UpdateJobExecutionResponse", "executionState") => Some(JOB_EXECUTION_STATE_SHAPE),
        _ => None,
    }
}

/// The shape of each structure a member of a response document targets. Taking the
/// member list from the shape's own generated [`Schema`] is what keeps this file out of
/// the way of a model change: a member added to `JobExecution` or `JobExecutionState`
/// reaches the wire on its own.
static JOB_EXECUTION_SHAPE: &Schema = &JobExecution::SCHEMA;
static JOB_EXECUTION_STATE_SHAPE: &Schema = &JobExecutionState::SCHEMA;

/// The members of `shape` this service does not send at all, so a completed structure
/// leaves them out of the body instead of reporting them as `null`.
///
/// `JobExecution$thingName` and the `statusDetails` of both shapes are the two
/// omissions `crate::state::to_job_execution` documents: every device-facing operation
/// names the thing in its own URI, and an execution carrying no name/value pair reports
/// no map at all. Both are pinned by recorded steps of `simparity-wire-aws-v1` (the
/// `$next` step and the read of `iotjobsdata-job-execution-queue-poll-and-start`, which
/// diverged on `$.execution.thingName` while the projection echoed it). Every OTHER
/// member of those shapes is completed.
fn unsent_members(shape: &Schema) -> &'static [&'static str] {
    match shape.shape_id().shape_name() {
        "JobExecution" => &["thingName", "statusDetails"],
        "JobExecutionState" => &["statusDetails"],
        _ => &[],
    }
}

/// A structure written so that every member of `shape` reaches the wire: the ones the
/// value writes itself, and the rest as JSON `null`.
///
/// This is what [`ResponseDocument`] is for the response document, applied at any
/// depth — the value's own [`SerializableStruct`] impl still writes the members, so no
/// shape's member list is re-stated anywhere.
struct CompleteStruct<'v, 'q> {
    value: &'v dyn SerializableStruct,
    shape: &'q Schema,
}

impl SerializableStruct for CompleteStruct<'_, '_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let mut completing = Complete {
            inner: serializer,
            shape: Some(self.shape),
            written: Vec::new(),
        };
        self.value.serialize_members(&mut completing)?;
        completing.fill_absent()
    }
}

/// A [`ShapeSerializer`] that notes which members the value wrote and passes every
/// write straight through, so that [`Complete::fill_absent`] can write JSON `null` for
/// the members that never reached it — re-wrapping the structures, lists and maps below
/// it so one rule holds for the whole document.
///
/// It wraps the serializer the framework hands to `serialize_members`, so a member the
/// value DOES write is written exactly as the model says — including an `@httpHeader`
/// or `@httpResponseCode` binding, which the framework's own binder below this
/// serializer still routes.
struct Complete<'s, 'q> {
    inner: &'s mut dyn ShapeSerializer,
    /// The shape whose members this completes, or `None` while the writes going
    /// through are a list's elements or a map's entries rather than one structure's
    /// members — there is no member list to complete against there, but a structure
    /// among those elements is still completed against its own.
    shape: Option<&'q Schema>,
    /// The `member_index` of every member the value wrote — the identity the
    /// generated schema gives a member, so noting one allocates nothing.
    written: Vec<usize>,
}

impl Complete<'_, '_> {
    /// Note that the value wrote `member`, whatever it wrote for it.
    fn written_member(&mut self, member: &Schema) {
        if let Some(index) = member.member_index() {
            self.written.push(index);
        }
    }

    /// The shape to complete a nested structure against, or `None` for one this
    /// service writes exactly as the value built it.
    ///
    /// `written` is the schema the structure's write carried: a SHAPE schema for a
    /// list element (no member index, and it carries the shape's members), a MEMBER
    /// schema for a structure member (whose target has to be resolved).
    fn nested_shape<'a>(&self, written: &'a Schema) -> Option<&'a Schema> {
        if written.member_index().is_none() {
            return (!written.members().is_empty()).then_some(written);
        }
        nested_target(self.shape?, written)
    }

    /// Write JSON `null` for every member of the shape being completed that the value
    /// wrote nothing for, apart from the members this service does not send.
    fn fill_absent(&mut self) -> ::std::result::Result<(), SerdeError> {
        let Some(shape) = self.shape else {
            return Ok(());
        };
        let unsent = unsent_members(shape);
        for member in shape.members() {
            let Some(index) = member.member_index() else {
                continue;
            };
            if self.written.contains(&index) {
                continue;
            }
            if member.member_name().is_some_and(|name| unsent.contains(&name)) {
                continue;
            }
            self.inner.write_null(member)?;
        }
        Ok(())
    }
}

/// Emit the [`ShapeSerializer`] methods that write a struct member: the write the
/// value asked for, plus a note that the member reached the wire.
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

impl ShapeSerializer for Complete<'_, '_> {
    note_and_pass_through! {
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

    /// A nested structure is written as completely as the document carrying it: the
    /// value is re-wrapped against its own shape, so a member it has no value for is
    /// JSON `null` instead of a key the body leaves out.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        match self.nested_shape(schema) {
            Some(shape) => self
                .inner
                .write_struct(schema, &CompleteStruct { value, shape }),
            None => self.inner.write_struct(schema, value),
        }
    }

    /// A list's elements are written to the serializer this hands the closure, so the
    /// rule reaches the summaries inside `inProgressJobs` and `queuedJobs`.
    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> ::std::result::Result<(), SerdeError>,
    ) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        self.inner.write_list(schema, &|elements| {
            write_elements(&mut Complete {
                inner: elements,
                shape: None,
                written: Vec::new(),
            })
        })
    }

    /// A map's entries, for the same reason as a list's elements.
    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> ::std::result::Result<(), SerdeError>,
    ) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        self.inner.write_map(schema, &|entries| {
            write_entries(&mut Complete {
                inner: entries,
                shape: None,
                written: Vec::new(),
            })
        })
    }
}
