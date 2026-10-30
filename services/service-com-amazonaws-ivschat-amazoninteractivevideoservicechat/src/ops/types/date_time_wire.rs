// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec one structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.ivschat#Time`, which the model marks `@timestampFormat("date-time")`
//! (`smithy_query(query="shape", shape="com.amazonaws.ivschat#Time")` reports
//! `"timestampFormat": "date-time"`, and each member's own doc says the value "is returned
//! as a string"). On the restJson1 wire this service speaks, that makes the ISO 8601 /
//! RFC 3339 STRING form part of the contract. The JSON codec decides the rendering per
//! member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is hand-written
//! service code). So a macro-built `date-time` member schema reports NO format and every
//! such member went onto the wire as a bare epoch number — not even the modelled JSON
//! type, so an SDK whose generated deserializer expects a `date-time` string cannot read
//! it.
//!
//! `vendor/` is read-only from inside the emulator, so the compensation lives here: the
//! macros below emit, for one structure, exactly what `#[smithy_macros::smithy(shape =
//! ...)]` emits — one `M_<MEMBER>` member schema per member (index = that member's position
//! in the model's member list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA`
//! static and the `SerializableStruct`/`DeserializableStruct` pair — plus the one chain call
//! the codegen omits, which [`ivschat_member_schema!`] attaches for the `date_time` member
//! kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, `@range`, the enum value set). The
//!   validating walk that enforces them runs on operation INPUTS only, and every structure
//!   built with these macros is output-only — no operation of this service takes a
//!   `CreateRoomResponse`, `GetRoomResponse`, `UpdateRoomResponse`, `RoomSummary`,
//!   `CreateChatTokenResponse`, `CreateLoggingConfigurationResponse`,
//!   `GetLoggingConfigurationResponse`, `UpdateLoggingConfigurationResponse` or a
//!   `LoggingConfigurationSummary` as its input — so the derive's captured constraints
//!   would be inert on them too.
//! * The `@http` chain. The codegen bakes method and URI onto a shape only when the shape
//!   is an operation's INPUT; none of these is.
//! * Map/list sub-member schemas (`.with_map_members(..)` / `.with_list_member(..)`). Only
//!   the XML and query codecs read them, to name an element; this service's sole protocol
//!   is restJson1.
//! * `@xmlName`/`@jsonName`. No member of the shapes below declares either: each one's
//!   wire name IS its Smithy member name.
//! * A `@required`/`@default` presence split. Every member of every shape below is
//!   optional in the model, so each one is written only when it is `Some` and read into an
//!   `Option`.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand — `services/service-com-amazonaws-ivsrealtime-\
//! amazoninteractivevideoservicerealtime/src/ops/types/date_time_wire.rs`,
//! `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/types/\
//! date_time_wire.rs` and
//! `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/types/\
//! date_time_wire.rs` carry near-identical macros. Once `vendor/smithy-macros` emits
//! `with_timestamp_format`, delete these macros and the `ivschat_schema_and_codec!` blocks
//! and put `#[smithy_macros::smithy(shape = ...)]` back on the structures that carry one —
//! and delete those sibling copies in the same change.
//! `tests/all/date_time_members_wire_form.rs` pins the wire form, so it fails if the format
//! does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types`
//! through an `include!`, so a `macro_rules!` declared under it counts as macro-expanded
//! and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! ivschat_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
    (union_of) => {
        ::aws_smithy_schema::ShapeType::Union
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `integer`, `date_time`, `map_of_string`, `list_of_string`,
/// `struct_of(T)` or `union_of(T)`.
macro_rules! ivschat_member_schema {
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
    // A `struct_of(T)`/`union_of(T)` entry's inner type names the Rust type the READ arm
    // deserializes into; no part of a member SCHEMA is built from it, so the trailing
    // group is matched and dropped rather than bound.
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $($element:tt)*) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            ivschat_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// map writes each key and value, and a list of scalars each element, against the MAP or
/// LIST member's own schema, and a structure or union member writes the whole value against
/// that member's schema.
macro_rules! ivschat_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, integer) => { $s.write_integer($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    // As in the schema macro, the inner type is the READ arm's; the write is the same call
    // for either kind, so the trailing group is matched and dropped.
    ($s:ident, $m:expr, $v:expr, struct_of $element:tt) => { $s.write_struct($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, union_of $element:tt) => { $s.write_struct($m, $v)?; };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Element and entry COUNTS are bounded by the request body the server already reads with
/// a cap, and nesting DEPTH by the codec itself: the vendored JSON deserializer takes a
/// depth guard at the entry of `read_struct`, `read_list` and `read_map` and refuses a body
/// nesting deeper than its `MAX_DEPTH`.
macro_rules! ivschat_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, integer) => { $d.read_integer($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<
            ::std::string::String,
            ::std::string::String,
        > = ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            __out.insert(__k, __dm.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
    ($d:ident, $m:expr, union_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member list:
/// everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape, with
/// `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "wireName", index, kind)`. The entries must be
/// given in the model's member order, which is the order `smithy_query(query="shape",
/// shape=...)` lists them in and the order each struct declares its fields in.
macro_rules! ivschat_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    ivschat_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    if let ::std::option::Option::Some(__v) = self.$field.as_ref() {
                        ivschat_write_member!(s, &Self::$cname, __v, $($kind)+);
                    }
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
                                    ivschat_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self { $( $field ),+ })
            }
        }
    };
}
