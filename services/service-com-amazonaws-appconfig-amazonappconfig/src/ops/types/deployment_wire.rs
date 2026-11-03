// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer a Deployment document needs, because its date-time
//! members cannot be rendered from a macro-built schema.
//!
//! WHY THIS MODULE EXISTS. `Deployment$StartedAt`, `Deployment$CompletedAt` and
//! `DeploymentEvent$OccurredAt` all target `com.amazonaws.appconfig#Iso8601DateTime`, a
//! `timestamp` shape carrying `@timestampFormat("date-time")`
//! (`smithy_query(query="shape", shape="Iso8601DateTime",
//! model_path="services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json")`
//! reports `"timestampFormat": "date-time"`). On the restJson1 wire this service speaks,
//! that makes the RFC 3339 / ISO-8601 STRING form part of the contract. The JSON codec
//! decides the rendering per member from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`) and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`, the XML/JSON
//! names and the HTTP bindings onto each member schema it emits, but it never emits
//! `.with_timestamp_format(..)` (grep the tree: every call site is hand-written service
//! code). So a macro-built `date-time` member schema reports NO format and every such
//! member went onto the wire as a bare epoch number — a type no generated SDK client can
//! deserialize into the modelled `date-time`.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA`
//! alias and the `SerializableStruct` impl — plus the one chain call the codegen omits,
//! which [`ac_member_schema`] attaches for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@range`, `@pattern`). The validating walk that enforces
//!   them runs on operation INPUTS only (`aws_smithy_schema_server::constraints::validate`,
//!   which `vendor/smithy-server-runtime/src/operation.rs` calls once on the deserialized
//!   input before the handler runs). Every structure built with these macros is
//!   output-only, so the macro's captured constraints would be inert here too.
//! * `DeserializableStruct`. These shapes are operation OUTPUTS, which the server only
//!   ever serializes (`Operation::Output` requires `SerializableStruct`; only `Input` is
//!   deserialized), so a hand-written read path would be untested code with no caller.
//! * The `@http` chain. It is baked onto a shape only when the shape is an operation's
//!   INPUT; neither of these is.
//! * `@xmlName`/`@jsonName`. No member of these shapes declares either, and this service's
//!   sole protocol is restJson1.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand —
//! `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/types/date_time_wire.rs`,
//! `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/ops/types/date_time_wire.rs`
//! and `services/service-com-amazonaws-account-account/src/ops/types/get_account_information_types.rs`
//! carry near-identical hand-written schemas, and the generator gap is filed as
//! `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md`.
//! Once `vendor/smithy-macros` emits `with_timestamp_format`, delete these macros and put
//! `#[smithy_macros::smithy(shape = ...)]` back on the structures that use them.
//! `tests/all/deployment_wire_fidelity.rs` pins the wire form, so it fails if the
//! attribute still drops it.
//!
//! The structures that need these macros live in `shared_types` and in
//! `start_deployment_types`, so `types/mod.rs` pulls this module in with `#[macro_use]` —
//! ahead of those modules, which is what puts the macros in their textual scope. They are
//! NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types` through an `include!`, so a
//! `macro_rules!` declared under it counts as macro-expanded and cannot be named by an
//! absolute `$crate::` path (rust-lang #52234).
//!
//! [`ac_schema_and_codec`] is not the `Deployment` shape's: every shape of this service
//! with a `date-time` member needs it, so `types/experiment_definition_wire.rs` states the
//! `ExperimentDefinition` shape's member list against the same macros.

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! ac_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (float) => {
        ::aws_smithy_schema::ShapeType::Float
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (list_of) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `integer`, `float`, `date_time`, `list_of(T)` or `struct_of(T)`.
macro_rules! ac_member_schema {
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $wire,
            $idx,
        )
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            ac_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// list of structures writes each element against the element shape's own `SCHEMA`, so the
/// element's member schemas (and the `date-time` format on them) are the ones that reach
/// the wire.
macro_rules! ac_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, integer) => { $s.write_integer($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, float) => { $s.write_float($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, list_of($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    // A map member writes each key and each value against the MAP member's own schema,
    // which is what the vendored codegen's `emit_write` does for `ShapeKind::Map`.
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    // A structure-typed member is written against the MEMBER schema, which is where the
    // codec reads the wire name from; the value's own member schemas write what is inside
    // it. The vendored codegen's `emit_write` does exactly this for `ShapeKind::Structure`.
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
}

/// Write a member only when the value is there: an `Option` one when it is `Some`, a
/// non-`Option` one (`@default`, which AWS always renders) always — the vendored codegen's
/// own `is_option` split.
macro_rules! ac_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, def, $($kind:tt)+) => {
        ac_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            ac_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The schema and the serializer for one structure, from its member list: what
/// `#[smithy_macros::smithy(shape = ...)]` emits for an output shape, with
/// `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "WireName", index, def|opt, kind)`; `def` is a
/// member carrying `@default` (a bare Rust field) and `opt` an `Option` one. The entries
/// must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! ac_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    ac_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+],
                );
        }

        pub static $schema_static: &::aws_smithy_schema::Schema = &$ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    ac_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }
    };
}

/// The `Deployment` shape's schema and serializer, for one of the Rust structs that
/// carries it.
///
/// `StartDeployment` and `GetDeployment` both answer `com.amazonaws.appconfig#Deployment`
/// and the scaffold declares one Rust struct per operation, so the member list is stated
/// ONCE here rather than once per struct: two documents of the same shape cannot then
/// disagree about a member, an order or a wire form.
///
/// Member set, order and requiredness from `smithy_query(query="shape",
/// shape="Deployment")`, plus `Type` — see the comment on its entry.
macro_rules! ac_deployment_shape {
    ($ty:ident, $schema_static:ident) => {
        ac_schema_and_codec!(
            $ty,
            $schema_static,
            "com.amazonaws.appconfig#Deployment",
            "com.amazonaws.appconfig",
            "Deployment",
            (M_APPLICATION_ID, application_id, "ApplicationId", 0, opt, string),
            (M_ENVIRONMENT_ID, environment_id, "EnvironmentId", 1, opt, string),
            (
                M_DEPLOYMENT_STRATEGY_ID,
                deployment_strategy_id,
                "DeploymentStrategyId",
                2,
                opt,
                string
            ),
            (
                M_CONFIGURATION_PROFILE_ID,
                configuration_profile_id,
                "ConfigurationProfileId",
                3,
                opt,
                string
            ),
            (M_DEPLOYMENT_NUMBER, deployment_number, "DeploymentNumber", 4, def, integer),
            (M_CONFIGURATION_NAME, configuration_name, "ConfigurationName", 5, opt, string),
            (
                M_CONFIGURATION_LOCATION_URI,
                configuration_location_uri,
                "ConfigurationLocationUri",
                6,
                opt,
                string
            ),
            (
                M_CONFIGURATION_VERSION,
                configuration_version,
                "ConfigurationVersion",
                7,
                opt,
                string
            ),
            (M_DESCRIPTION, description, "Description", 8, opt, string),
            (
                M_DEPLOYMENT_DURATION_IN_MINUTES,
                deployment_duration_in_minutes,
                "DeploymentDurationInMinutes",
                9,
                def,
                integer
            ),
            (M_GROWTH_TYPE, growth_type, "GrowthType", 10, opt, string),
            (M_GROWTH_FACTOR, growth_factor, "GrowthFactor", 11, opt, float),
            (
                M_FINAL_BAKE_TIME_IN_MINUTES,
                final_bake_time_in_minutes,
                "FinalBakeTimeInMinutes",
                12,
                def,
                integer
            ),
            (M_STATE, state, "State", 13, opt, string),
            (M_EVENT_LOG, event_log, "EventLog", 14, opt, list_of(DeploymentEvent)),
            (M_PERCENTAGE_COMPLETE, percentage_complete, "PercentageComplete", 15, opt, float),
            (M_STARTED_AT, started_at, "StartedAt", 16, opt, date_time),
            (M_COMPLETED_AT, completed_at, "CompletedAt", 17, opt, date_time),
            (
                M_APPLIED_EXTENSIONS,
                applied_extensions,
                "AppliedExtensions",
                18,
                opt,
                list_of(AppliedExtension)
            ),
            (M_KMS_KEY_ARN, kms_key_arn, "KmsKeyArn", 19, opt, string),
            (M_KMS_KEY_IDENTIFIER, kms_key_identifier, "KmsKeyIdentifier", 20, opt, string),
            (M_VERSION_LABEL, version_label, "VersionLabel", 21, opt, string),
            // `Type` is the one member here the ON-DISK model does not declare on
            // `Deployment` while the recorded service answers it: the recorded
            // `StartDeployment`/`GetDeployment` documents carry `"Type": "USER"`. The model
            // does carry the member on the sibling summary shape
            // (`smithy_query(query="shape", shape="DeploymentSummary")` lists
            // `Type -> DeploymentType`) and the enum it targets (`DeploymentType`:
            // `USER | MANAGED`), so the wire name, the position after the model's own
            // members and the value space are the model's; only this shape's member list is
            // behind the service. A string on the wire, like every other enum member.
            (M_TYPE, r#type, "Type", 22, opt, string),
        );
    };
}
