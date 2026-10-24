// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer a structure needs when its wire form is not what
//! `#[smithy_macros::smithy(shape = ...)]` derives — because a member carries
//! `@timestampFormat("date-time")`, or because the recorded wire carries a member the
//! model does not declare.
//!
//! WHY THIS MODULE EXISTS.
//!
//! 1. TIMESTAMP FORMAT. This service's timestamp members are marked
//!    `@timestampFormat("date-time")` on the member
//!    (`smithy_query(query="operation",operation="CreateAgentSpace,CreateTargetDomain",
//!    model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`
//!    and `smithy_query(query="shape",shape="AgentSpace,TargetDomain,SecurityRequirementPackSummary,PrivateConnectionSummary")`),
//!    so on this service's restJson1 wire each one is an RFC 3339 / ISO-8601 STRING. The
//!    JSON codec decides the rendering per member from the member schema's own format
//!    (`vendor/smithy-schema-server/src/json.rs`: `write_timestamp` reads
//!    `Schema::timestamp_format()`) and falls back to the protocol default — epoch
//!    seconds, a JSON NUMBER — when the member schema reports none.
//!    `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/
//!    `@uniqueItems`, `@xmlName`, `@jsonName` and the HTTP bindings onto every member
//!    schema it emits, but it never emits `.with_timestamp_format(..)` (grep the tree:
//!    every call site is hand-written service code), so a derived `date-time` member
//!    goes onto the wire as a bare epoch number — not even the modelled JSON type, so an
//!    SDK deserializing it faults. The gap is filed as
//!    `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md`;
//!    `vendor/` is read-only from a service crate, so the compensation lives here.
//!
//! 2. RECORDED-BUT-UNMODELLED MEMBERS. The recorded wire of this service carries
//!    members its model does not declare (an agent space's `id`/`ownerId`/`arn` and its
//!    resolved `targetDomains`, a target domain's `status`). A derived struct cannot
//!    carry them at all: the derive refuses a Rust field that matches no Smithy member.
//!    They are declared through [`sa_recorded_member_schema`], which puts them in the
//!    `com.amazonaws.securityagent.recorded` namespace rather than the service's own —
//!    the member is NOT in the model, so its shape id must not claim it is, nor collide
//!    with a member the model may declare later.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * `DeserializableStruct`. Every structure built with these macros is an operation
//!   OUTPUT or a shape reached only from one, and the server only ever SERIALIZES those
//!   (`smithy_server_runtime::Operation` requires `SerializableStruct` of `Output` and
//!   `DeserializableStruct` of `Input` alone), so a hand-written read path would be
//!   untested code with no caller.
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces
//!   them runs on operation INPUTS only, so the derive's captured constraints are inert
//!   on an output shape too.
//! * Map/list sub-member schemas (`.with_list_member(..)`): only the XML codecs read
//!   them, and this service's sole protocol is restJson1.
//!
//! REMOVE the timestamp half when the vendored codegen learns
//! `smithy.api#timestampFormat`: put `#[smithy_macros::smithy(shape = ...)]` back on
//! every structure below that carries no recorded-but-unmodelled member.
//! `tests/all/recorded_wire_form.rs` pins the wire form, so it fails if the format stops
//! reaching the wire.
//!
//! The structures that need these macros are spread across this service's per-operation
//! `types/` modules, where each shape is declared exactly once, so `types/mod.rs` pulls
//! this module in with `#[macro_use]` AHEAD of them — which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches
//! `types` through an `include!`, so a `macro_rules!` declared under it counts as
//! macro-expanded and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! sa_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of_struct) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
}

/// One MODELLED member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `date_time`, `list_of_string`, `map_of_string`,
/// `list_of_struct(T)` or `struct_of(T)`.
macro_rules! sa_member_schema {
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
        // The one chain call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            sa_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// One RECORDED-BUT-UNMODELLED member's schema. Identical to [`sa_member_schema`]
/// except for the namespace: the member is not in the model, so it is declared in
/// `com.amazonaws.securityagent.recorded` instead of the service's own namespace.
macro_rules! sa_recorded_member_schema {
    ($local:literal, $wire:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!(
                    "com.amazonaws.securityagent.recorded#",
                    $local,
                    "$",
                    $wire
                ),
                "com.amazonaws.securityagent.recorded",
                $wire,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $wire,
            $idx,
        )
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!(
                    "com.amazonaws.securityagent.recorded#",
                    $local,
                    "$",
                    $wire
                ),
                "com.amazonaws.securityagent.recorded",
                $wire,
            ),
            sa_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that
/// kind: a list of structures writes each element against the element shape's own
/// `SCHEMA`, and a map writes the key and the value against the member schema.
macro_rules! sa_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __entry) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __entry.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, list_of_struct($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
}

/// Write a member only when the value is there: a non-`Option` field always, an
/// `Option` one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! sa_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        sa_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            sa_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The schema and the serializer for one output structure, from its member list.
///
/// Each member entry is `(CONST, field, "wireName", index, req|opt, kind)`; `req` is a
/// bare Rust field and `opt` an `Option` one. The `modelled:` entries must be given in
/// the model's member order — the order `smithy_query(query="shape", shape=...)` lists
/// them in — and the `recorded:` entries, which the model does not declare at all,
/// follow them with the next indices.
macro_rules! sa_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        modelled: [
            $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
        ],
        recorded: [
            $( ( $rname:ident, $rfield:ident, $rwire:literal, $ridx:literal, $rpresence:tt, $($rkind:tt)+ ) ),* $(,)?
        ] $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    sa_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+
            $(
                pub const $rname: ::aws_smithy_schema::Schema =
                    sa_recorded_member_schema!($local, $rwire, $ridx, $($rkind)+);
            )*

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+ $(&Self::$rname,)*],
                );
        }

        pub static $schema_static: &::aws_smithy_schema::Schema = &$ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    sa_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                $(
                    sa_write_present_member!(
                        s, self, Self::$rname, $rfield, $rpresence, $($rkind)+
                    );
                )*
                Ok(())
            }
        }
    };
}
