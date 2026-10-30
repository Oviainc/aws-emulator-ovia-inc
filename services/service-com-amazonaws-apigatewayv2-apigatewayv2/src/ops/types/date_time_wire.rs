// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec one structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.apigatewayv2#__timestampIso8601`, a `timestamp` shape carrying
//! `@timestampFormat("date-time")` (`smithy_query(query="shape",
//! shape="__timestampIso8601")` reports `"timestampFormat": "date-time"`, and each member
//! repeats it). On the restJson1 wire this service speaks, that makes the RFC 3339 /
//! ISO-8601 STRING form part of the contract. The JSON codec decides the rendering per
//! member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is hand-written
//! service code). So a macro-built `date-time` member schema reports NO format and every
//! such member went onto the wire as a bare epoch number — a type no generated SDK client
//! can deserialize into the modelled `date-time`.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list, name and `@jsonName` as the model spells them), a `SCHEMA`
//! naming them in that order, the `<SHAPE>_SCHEMA` static, and the
//! `SerializableStruct`/`DeserializableStruct` pair — plus the one chain call the codegen
//! omits, which [`apigwv2_member_schema`] attaches for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only — `aws_smithy_schema_server::validate_to_message`, which
//!   `smithy_service` calls once on the deserialized input before the handler runs. Every
//!   structure built with these macros is a response shape, a list element of one, or
//!   `PortalTheme`, whose own two members (a structure and the timestamp) carry no
//!   constraint trait at all — the constrained members of a portal's content sit on
//!   `CustomColors` and `PortalContent`, which keep the derive.
//! * The `@http` chain. The vendored codegen bakes method and URI onto a shape only when
//!   the shape is an operation's INPUT, and never bakes the success status onto a shape at
//!   all; none of these is an input.
//! * Map/list sub-member schemas (`.with_map_members(..)` / `.with_list_member(..)`). Only
//!   the XML codecs read them, to name an element; this service's sole protocol is
//!   restJson1.
//! * `@xmlName`. No member of this service's model declares one.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand — `services/service-com-amazonaws-kafka-kafka/src/ops/types/
//! date_time_wire.rs` and
//! `services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/
//! types/shared_types.rs` carry near-identical helpers. Once `vendor/smithy-macros` emits
//! `with_timestamp_format`, delete these macros and the `apigwv2_schema_and_codec!` blocks
//! and put `#[smithy_macros::smithy(shape = ...)]` back on the structures that carry one —
//! and delete those sibling copies in the same change.
//! `tests/all/iso8601_timestamp_members.rs` pins the wire form, so it fails if the format
//! does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types`
//! through an `include!`, so a `macro_rules!` declared under it counts as macro-expanded
//! and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident` — an `enum` shape is `String`, as that function has it.
macro_rules! apigwv2_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$local` is the enclosing shape's local name, `$member` the member as the model spells
/// it, `$json` its `@jsonName` wire key and `$idx` its position in the model's member
/// list. `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `date_time`, `list_of_string`, `map_of_string` or `struct_of(T)`.
macro_rules! apigwv2_member_schema {
    ($local:literal, $member:literal, $json:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.apigatewayv2#", $local, "$", $member),
                "com.amazonaws.apigatewayv2",
                $member,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $member,
            $idx,
        )
        .with_json_name($json)
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($local:literal, $member:literal, $json:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.apigatewayv2#", $local, "$", $member),
                "com.amazonaws.apigatewayv2",
                $member,
            ),
            apigwv2_shape_type!($kind),
            $member,
            $idx,
        )
        .with_json_name($json)
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind.
macro_rules! apigwv2_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    // A map of strings writes each entry as the vendored codegen's `emit_write` does for
    // `ShapeKind::Map`: the key as a string, then the value, both against the MEMBER's
    // schema.
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => { $s.write_struct($m, $v)?; };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Element and entry COUNTS are bounded by the request body the server already reads with
/// a cap, and nesting DEPTH by the codec itself: the vendored JSON deserializer takes a
/// depth guard at the entry of `read_struct`, `read_list` and `read_map` and refuses a
/// body nesting deeper than its `MAX_DEPTH`.
macro_rules! apigwv2_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<
            ::std::string::String,
            ::std::string::String,
        > = ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            let __value = __dm.read_string($m)?;
            __out.insert(__k, __value);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when the value is there: a `@required` member (a bare Rust field)
/// always, an `Option` one when it is `Some` — the vendored codegen's own `is_option`
/// split.
macro_rules! apigwv2_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        apigwv2_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            apigwv2_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! apigwv2_member_value {
    ($slot:ident, $json:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($json))?
    };
    ($slot:ident, $json:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for a response shape,
/// with `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "Member", "jsonName", index, req|opt, kind)`;
/// `req` is a `@required` member (a bare Rust field, refused when absent on the wire) and
/// `opt` an `Option` one. The entries must be given in the model's member order, which is
/// the order `smithy_query(query="shape", shape=...)` lists them in and the order each
/// struct declares its fields in.
macro_rules! apigwv2_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $local:literal,
        $( (
            $cname:ident, $field:ident, $member:literal, $json:literal, $idx:literal,
            $presence:tt, $($kind:tt)+
        ) ),+ $(,)?
    ) => {
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    apigwv2_member_schema!($local, $member, $json, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        concat!("com.amazonaws.apigatewayv2#", $local),
                        "com.amazonaws.apigatewayv2",
                        $local,
                    ),
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
                    apigwv2_write_present_member!(
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
                                    apigwv2_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: apigwv2_member_value!($field, $json, $presence) ),+
                })
            }
        }
    };
}
