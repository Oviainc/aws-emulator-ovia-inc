// SPDX-License-Identifier: CC0-1.0

//! How this service puts a persisted [`App`] or [`Branch`] on the wire.
//!
//! Amplify answers with a COMPLETE record. Two properties of that record cannot be
//! expressed by the generated serializer alone, and both are what this module adds:
//!
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
//! own [`SerializableStruct`] impl rather than re-listing its members, so a member
//! added to the model reaches the wire without touching this file.
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
        let mut projecting = Unvalued {
            inner: serializer,
            unvalued: &unvalued,
        };
        self.0.serialize_members(&mut projecting)
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

    /// The branch as a REPORTING operation (`GetBranch`, `ListBranches`) answers it:
    /// the stored record plus the tag map every read carries. The stored record is
    /// untouched.
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
        let mut projecting = Unvalued {
            inner: serializer,
            unvalued: &unvalued,
        };
        self.0.serialize_members(&mut projecting)
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

/// A [`ShapeSerializer`] that writes the members in `unvalued` as JSON `null` and
/// passes every other write straight through.
///
/// It wraps the serializer the framework hands to `serialize_members`, so the record
/// writes its members exactly as the model says and only the named ones are
/// re-projected. A nested shape is written by the inner serializer, so nothing below
/// this structure is affected.
struct Unvalued<'s, 'n> {
    inner: &'s mut dyn ShapeSerializer,
    unvalued: &'n [&'static str],
}

impl Unvalued<'_, '_> {
    /// Is `member` one of the members this record has no value for?
    fn has_no_value(&self, member: &Schema) -> bool {
        member
            .member_name()
            .is_some_and(|name| self.unvalued.contains(&name))
    }
}

/// Emit the [`ShapeSerializer`] methods that write a struct member: `null` for a
/// member with no value, otherwise the write the record asked for.
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
                if self.has_no_value(schema) {
                    return self.inner.write_null(schema);
                }
                self.inner.$method(schema, $($arg),*)
            }
        )*
    };
}

impl ShapeSerializer for Unvalued<'_, '_> {
    null_or_pass_through! {
        fn write_struct(value: &dyn SerializableStruct);
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

    fn write_null(&mut self, schema: &Schema) -> ::std::result::Result<(), SerdeError> {
        self.inner.write_null(schema)
    }
}
