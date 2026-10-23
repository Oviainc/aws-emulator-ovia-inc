// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp this service models targets
//! `com.amazonaws.lambdacore#CoreTimestamp`, which carries `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="CoreTimestamp")` reports
//! `"timestampFormat": "date-time"`, and `query="shape"` repeats it on
//! `GetNetworkConnectorResponse$LastModified`, `UpdateNetworkConnectorResponse$LastModified`,
//! `NetworkConnectorSummary$LastModified` and `NetworkConnectorVersionSummary$LastModified`).
//! On this service's restJson1 wire that makes the RFC 3339 / ISO-8601 STRING form part of
//! the contract. The JSON codec decides the rendering per member, from the member schema's
//! own format (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is hand-written
//! service code). So a macro-built `date-time` member schema reports NO format and every
//! such member goes onto the wire as a bare epoch number — a type no generated SDK client
//! can deserialize into the modelled `date-time`.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros below
//! emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]` emits —
//! one member schema per member (index = that member's position in the model's member
//! list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA` static and the
//! `SerializableStruct`/`DeserializableStruct` pair — plus the one chain call the codegen
//! omits, which [`lambdacore_member_schema!`] attaches for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, `@range`). The validating walk that enforces
//!   them runs on operation INPUTS only (`aws_smithy_schema_server::validate_to_message`,
//!   which `smithy_service` calls once on the deserialized input before the handler runs).
//!   Every structure built with these macros is output-only, so the macro's captured
//!   constraints would be inert here too.
//! * `@http` bindings. The vendored codegen attaches those to an INPUT shape's schema only
//!   (see its `http_struct_chain`), and no member of the shapes below is bound to a header,
//!   a label or a query parameter.
//! * `@jsonName`/`@xmlName`. No member of this service's model declares either: a member's
//!   wire key IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect. It is filed as
//! `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md`
//! (and `…20261021T223000Z-scaffold-codegen-drops-timestampformat-from-member-schemas.md`),
//! and near-identical macros live in
//! `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/types/date_time_wire.rs`
//! and `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/types/date_time_wire.rs`.
//! Once `vendor/smithy-macros` emits `with_timestamp_format`, delete these macros and the
//! `lambdacore_schema_and_codec!` blocks and put `#[smithy_macros::smithy(shape = ...)]`
//! back on the structures that carry one. `tests/all/timestamp_and_state_reason_wire.rs`
//! pins the wire form, so it fails if the format stops reaching the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types`
//! through an `include!`, so a `macro_rules!` declared under it counts as macro-expanded
//! and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident` (an `enum` shape is string-encoded, so `String`).
macro_rules! lambdacore_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (long) => {
        ::aws_smithy_schema::ShapeType::Long
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (union_of) => {
        ::aws_smithy_schema::ShapeType::Union
    };
}

/// One member's schema, as `#[smithy_macros::smithy(shape = ...)]` emits it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `long`, `date_time` or `union_of(T)`.
macro_rules! lambdacore_member_schema {
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
            lambdacore_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind.
macro_rules! lambdacore_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, long) => {
        $s.write_long($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, union_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Nesting DEPTH is bounded by the codec itself: the vendored JSON deserializer takes a
/// depth guard at the entry of `read_struct`, `read_list` and `read_map` and refuses a body
/// nesting deeper than its `MAX_DEPTH`.
macro_rules! lambdacore_read_member {
    ($d:ident, $m:expr, string) => {
        $d.read_string($m)?
    };
    ($d:ident, $m:expr, long) => {
        $d.read_long($m)?
    };
    ($d:ident, $m:expr, date_time) => {
        $d.read_timestamp($m)?
    };
    ($d:ident, $m:expr, union_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! lambdacore_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        lambdacore_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            lambdacore_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! lambdacore_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape,
/// with `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "WireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! lambdacore_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    lambdacore_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    lambdacore_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for $ty {
            fn deserialize(
                d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                $( let mut $field = ::std::option::Option::None; )+
                d.read_struct(&Self::SCHEMA, &mut |member, d| {
                    match member.member_index() {
                        $(
                            ::std::option::Option::Some($idx) => {
                                $field = ::std::option::Option::Some(
                                    lambdacore_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: lambdacore_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
