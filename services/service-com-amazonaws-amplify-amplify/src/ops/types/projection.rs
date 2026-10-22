// SPDX-License-Identifier: CC0-1.0

//! How this service puts a persisted [`App`] or [`Branch`] on the wire.
//!
//! Amplify answers with a COMPLETE record. Three properties of that record cannot be
//! expressed by the generated serializer alone, and all three are what this module
//! adds:
//!
//! * **Every modelled member reaches the wire.** The generated serializer skips an
//!   `Option` member that holds `None`, so `Branch$buildSpec`,
//!   `Branch$sourceBranch`, `App$productionBranch` — every optional member a record
//!   has no value for — would be absent from the body, while Amplify sends the key
//!   with JSON `null`. A client dereferencing such a member gets `undefined` instead
//!   of `null`, and a list element ends up a different SHAPE from the same record
//!   fetched on its own. The projection therefore writes JSON `null` for every
//!   modelled member the record itself did not write, and completes the nested
//!   `Branch$backend` the same way (`{"stackArn": null}`). A structure the CALLER
//!   configures is the one exception — it is echoed with the members that
//!   configuration carries; see [`nested_shape`].
//! * **A member the service has no value for is JSON `null`.** `App$description`,
//!   `App$repository`, `App$environmentVariables`, `Branch$description`,
//!   `Branch$activeJobId`, `Branch$customDomains`, `Branch$environmentVariables` and
//!   `Branch$framework` are `@required` members
//!   (`smithy_query(query="shape",shape="App")` / `shape="Branch"`) whose `length`
//!   minimum is 0, so the generated record type holds each as a bare `String`,
//!   `Vec` or `HashMap` and the generated serializer writes that type's zero value —
//!   `""`, `[]`, `{}`. A client reading `""` where Amplify sends `null` draws the
//!   opposite conclusion from an emptiness test, so the projection writes the zero
//!   value of those members as `null` instead.
//! * **A read reports the resource's tag map.** `tags` is optional on both shapes,
//!   and a reporting operation answers with the map the resource carries — the empty
//!   map when it carries none — while the operation that wrote the record echoes only
//!   the tags the request set.
//!
//! Everything else about the walk stays generated: the projection wraps the record's
//! own [`SerializableStruct`] impl rather than re-listing its members, and the member
//! list it completes against is the record's own [`Schema`], so a member added to the
//! model reaches the wire without touching this file.
//!
//! Every operation that answers with an app or a branch — single record or list
//! element — answers through these two types, so a `ListBranches` element and a
//! `GetBranch` body cannot disagree about the same branch.

use super::*;

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_schema_server::DeserializableStruct;
use ::aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};

/// An [`App`] as every operation that returns one writes it.
#[derive(Debug, Clone)]
pub struct AppProjection(App);

impl AppProjection {
    /// The shape this projects is still `com.amazonaws.amplify#App`: the projection
    /// changes how a record is WRITTEN, never which shape it is. The generated
    /// serializer for a list of apps reads the element schema from this constant.
    pub const SCHEMA: Schema = App::SCHEMA;

    /// The app as the operation that WROTE the record answers it: the record exactly
    /// as stored, `tags` included only when the request that created it set them.
    pub fn written(app: App) -> Self {
        Self(app)
    }

    /// The app as a REPORTING operation (`GetApp`, `ListApps`) answers it: the stored
    /// record, plus the tag map every read carries — the empty map when the app has
    /// no tags.
    ///
    /// This fills the member on the OUTGOING copy only; the stored record is
    /// untouched, so a read changes nothing a later caller sees.
    pub fn reported(mut app: App) -> Self {
        app.tags = Some(app.tags.unwrap_or_default());
        Self(app)
    }

    /// The members this record has no value for, which Amplify reports as `null`.
    ///
    /// Each is `@required` in the model, so "no value" can only be held as the zero
    /// value of the member's type; reporting that zero value verbatim is what this
    /// projection exists to prevent. `platform`, `name`, `appId`, `appArn`,
    /// `defaultDomain` and the three `@required` booleans are NOT listed: the service
    /// determines a value for every one of them.
    fn unvalued_members(&self) -> Vec<&'static str> {
        let mut unvalued = Vec::new();
        if self.0.description.is_empty() {
            unvalued.push("description");
        }
        if self.0.repository.is_empty() {
            unvalued.push("repository");
        }
        if self.0.environment_variables.is_empty() {
            unvalued.push("environmentVariables");
        }
        unvalued
    }
}

impl SerializableStruct for AppProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let unvalued = self.unvalued_members();
        CompleteRecord {
            record: &self.0,
            shape: APP_SHAPE,
            unvalued: &unvalued,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for AppProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self(<App as DeserializableStruct>::deserialize(
            deserializer,
        )?))
    }
}

/// A [`Branch`] as every operation that returns one writes it.
#[derive(Debug, Clone)]
pub struct BranchProjection(Branch);

impl BranchProjection {
    /// The shape this projects is still `com.amazonaws.amplify#Branch`; see
    /// [`AppProjection::SCHEMA`].
    pub const SCHEMA: Schema = Branch::SCHEMA;

    /// The branch as the operation that WROTE the record answers it.
    pub fn written(branch: Branch) -> Self {
        Self(branch)
    }

    /// The branch as a REPORTING operation (`GetBranch`, `ListBranches`) or an
    /// UPDATE (`UpdateBranch`) answers it: the stored record plus the tag map every
    /// one of those answers carries. The stored record is untouched.
    pub fn reported(mut branch: Branch) -> Self {
        branch.tags = Some(branch.tags.unwrap_or_default());
        Self(branch)
    }

    /// The members this record has no value for, which Amplify reports as `null`.
    ///
    /// Each is `@required` in the model, so "no value" can only be held as the zero
    /// value of the member's type; reporting that zero value verbatim is what this
    /// projection exists to prevent. `stage`, `displayName`, `ttl`,
    /// `totalNumberOfJobs` and the `@required` booleans are NOT listed: the service
    /// determines a value for every one of them.
    fn unvalued_members(&self) -> Vec<&'static str> {
        let mut unvalued = Vec::new();
        if self.0.description.is_empty() {
            unvalued.push("description");
        }
        if self.0.active_job_id.is_empty() {
            unvalued.push("activeJobId");
        }
        if self.0.custom_domains.is_empty() {
            unvalued.push("customDomains");
        }
        if self.0.environment_variables.is_empty() {
            unvalued.push("environmentVariables");
        }
        if self.0.framework.is_empty() {
            unvalued.push("framework");
        }
        unvalued
    }
}

impl SerializableStruct for BranchProjection {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let unvalued = self.unvalued_members();
        CompleteRecord {
            record: &self.0,
            shape: BRANCH_SHAPE,
            unvalued: &unvalued,
        }
        .serialize_members(serializer)
    }
}

impl DeserializableStruct for BranchProjection {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, SerdeError> {
        Ok(Self(<Branch as DeserializableStruct>::deserialize(
            deserializer,
        )?))
    }
}

/// The shape schema of each structure this projection completes, as the member list
/// `Complete::fill_absent` walks. Taking it from the record's own generated
/// [`Schema`] is what keeps this file out of the way of a model change: a member
/// added to `App`, `Branch` or `Backend` appears here on its own.
static APP_SHAPE: &Schema = &App::SCHEMA;
static BRANCH_SHAPE: &Schema = &Branch::SCHEMA;
static BACKEND_SHAPE: &Schema = &Backend::SCHEMA;

/// The shape a STRUCTURE member of a completed record targets, for the members whose
/// target this projection completes too — `Branch$backend`, which Amplify answers as
/// `{"stackArn": null}` rather than `{}` for a branch whose Gen 2 deployment has not
/// provisioned a CloudFormation stack.
///
/// The lookup is keyed by the PARENT shape as well as the member name, so an entry
/// cannot mis-fire on a same-named member of another shape.
///
/// A structure the CALLER configures (`App$autoBranchCreationConfig`,
/// `App$cacheConfig`, `App$jobConfig`, `App$wafConfiguration`,
/// `App$productionBranch`) is NOT completed: it is echoed with the members the
/// configuration carries, which is what `tests/all/app_branch_updates.rs` pins and
/// what every recording of this service shows. A member whose target is not a
/// structure, or a structure inside a list (whose element member is named `member`),
/// answers `None` and is written exactly as the record wrote it.
fn nested_shape(parent: &Schema, member: &Schema) -> Option<&'static Schema> {
    match (parent.shape_id().shape_name(), member.member_name()?) {
        ("Branch", "backend") => Some(BACKEND_SHAPE),
        _ => None,
    }
}

/// `record`, written so that EVERY member of `shape` reaches the wire: the members
/// the record writes itself, the ones named in `unvalued` as JSON `null`, and the
/// ones it wrote nothing for as JSON `null` too.
struct CompleteRecord<'r> {
    record: &'r dyn SerializableStruct,
    shape: &'static Schema,
    unvalued: &'r [&'static str],
}

impl SerializableStruct for CompleteRecord<'_> {
    fn serialize_members(
        &self,
        serializer: &mut dyn ShapeSerializer,
    ) -> ::std::result::Result<(), SerdeError> {
        let mut completing = Complete {
            inner: serializer,
            shape: self.shape,
            unvalued: self.unvalued,
            written: Vec::new(),
        };
        self.record.serialize_members(&mut completing)?;
        completing.fill_absent()
    }
}

/// A [`ShapeSerializer`] that writes the members in `unvalued` as JSON `null`, notes
/// which members the record wrote, and passes every other write straight through.
///
/// It wraps the serializer the framework hands to `serialize_members`, so the record
/// writes its members exactly as the model says and only the named ones are
/// re-projected. A nested structure this service completes is re-wrapped (see
/// [`nested_shape`]); anything else below this structure is written by the inner
/// serializer untouched.
struct Complete<'s, 'n> {
    inner: &'s mut dyn ShapeSerializer,
    shape: &'static Schema,
    unvalued: &'n [&'static str],
    /// The `member_index` of every member the record wrote — the identity the
    /// generated schema gives a member, so noting one allocates nothing.
    written: Vec<usize>,
}

impl Complete<'_, '_> {
    /// Is `member` one of the members this record has no value for?
    fn has_no_value(&self, member: &Schema) -> bool {
        member
            .member_name()
            .is_some_and(|name| self.unvalued.contains(&name))
    }

    /// Note that the record wrote `member`, whatever it wrote for it.
    fn written_member(&mut self, member: &Schema) {
        if let Some(index) = member.member_index() {
            self.written.push(index);
        }
    }

    /// Write JSON `null` for every member of this record's shape that the record
    /// wrote nothing for.
    fn fill_absent(&mut self) -> ::std::result::Result<(), SerdeError> {
        let shape = self.shape;
        for member in shape.members() {
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

/// Emit the [`ShapeSerializer`] methods that write a struct member: `null` for a
/// member with no value, otherwise the write the record asked for — and in both cases
/// a note that the member reached the wire.
///
/// Every member write is routed through the same check — a member is a member
/// whatever its type — so the projection cannot be right for a string and wrong for
/// a list.
macro_rules! null_or_pass_through {
    ($( fn $method:ident($($arg:ident: $ty:ty),*); )*) => {
        $(
            fn $method(
                &mut self,
                schema: &Schema,
                $($arg: $ty),*
            ) -> ::std::result::Result<(), SerdeError> {
                self.written_member(schema);
                if self.has_no_value(schema) {
                    return self.inner.write_null(schema);
                }
                self.inner.$method(schema, $($arg),*)
            }
        )*
    };
}

impl ShapeSerializer for Complete<'_, '_> {
    null_or_pass_through! {
        fn write_list(
            write_elements: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
        fn write_map(
            write_entries: &dyn Fn(&mut dyn ShapeSerializer)
                -> ::std::result::Result<(), SerdeError>
        );
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
    }

    /// A nested structure is written as completely as the record carrying it: the
    /// value is re-wrapped in a [`CompleteRecord`] over its own shape, so its unset
    /// members are JSON `null` instead of missing keys.
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        if self.has_no_value(schema) {
            return self.inner.write_null(schema);
        }
        match nested_shape(self.shape, schema) {
            Some(shape) => self.inner.write_struct(
                schema,
                &CompleteRecord {
                    record: value,
                    shape,
                    unvalued: &[],
                },
            ),
            None => self.inner.write_struct(schema, value),
        }
    }

    fn write_null(&mut self, schema: &Schema) -> ::std::result::Result<(), SerdeError> {
        self.written_member(schema);
        self.inner.write_null(schema)
    }
}
