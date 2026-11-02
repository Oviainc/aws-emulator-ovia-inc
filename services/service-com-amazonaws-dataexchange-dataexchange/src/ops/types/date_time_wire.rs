// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer a structure of this service needs when one of its
//! members carries `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.dataexchange#Timestamp`, and that shape carries
//! `@timestampFormat("date-time")` —
//! `smithy_query(query="shape", shape="Timestamp,CreateDataSetResponse")` reports
//! `"timestampFormat": "date-time"` on the shape and on every member targeting it, and
//! each member's own doc says so in prose ("The date and time that the data set was
//! created, in ISO 8601 format"). On the restJson1 wire that makes the RFC 3339 STRING
//! form part of the contract, for `CreatedAt`, `UpdatedAt`, `RevokedAt`, `AcceptedAt`,
//! `EndsAt` and every `…ExpiresAt` member alike.
//!
//! The JSON codec decides the rendering per member from the MEMBER schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`) and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is
//! hand-written service code). So a macro-built `date-time` member schema reports NO
//! format and every such member went onto the wire as a bare epoch number — a value no
//! generated SDK client can deserialize into the modelled `date-time`, and not what AWS
//! answers.
//!
//! `vendor/` is read-only from inside the emulator, so the remedy is the per-shape one
//! this service already applies to the three notification-detail INPUT shapes (see
//! `send_data_set_notification_types`): write the shape's schema and codec out with the
//! modelled format. The macros below emit, for one structure, exactly what
//! `#[smithy_macros::smithy(shape = ...)]` emits for an output shape — one `M_<MEMBER>`
//! member schema per member (index = that member's position in the model's member list), a
//! `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA` static and the
//! `SerializableStruct` impl — plus the one chain call the codegen omits, which
//! [`dx_member_schema`] attaches for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT, for the reasons the same remedy states elsewhere in the
//! workspace (see
//! `services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/types/date_time_wire.rs`):
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only, and every structure built with these macros is
//!   output-only, so the macro's captured constraints would be inert here too.
//! * `DeserializableStruct`. Only `Operation::Input` is deserialized
//!   (`vendor/smithy-server-runtime/src/operation.rs`); an output shape is only ever
//!   serialized.
//! * Map/list sub-member schemas (`.with_list_member(..)`). Only the XML codecs read them,
//!   to name an element; this service's sole protocol is restJson1.
//! * `@xmlName`/`@jsonName`/HTTP bindings. No member of the shapes below declares any:
//!   each one's wire name IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat` and
//! put `#[smithy_macros::smithy(shape = ...)]` back on the structures below;
//! `tests/all/timestamp_wire_form.rs` pins the wire form, so it fails if the format stops
//! reaching the wire.
//!
//! The structures that need these macros are spread across this service's per-operation
//! `types/` modules, where each shape is declared exactly once, so `types/mod.rs` pulls
//! this module in with `#[macro_use]` — ahead of those modules, which is what puts these
//! macros in their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs`
//! reaches `types` through an `include!`, so a `macro_rules!` declared under it counts as
//! macro-expanded and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! dx_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (list_of) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `boolean`, `date_time`, `map_of_string`, `list_of(T)` or
/// `struct_of(T)`.
macro_rules! dx_member_schema {
    ($local:literal, $wire:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.dataexchange#", $local, "$", $wire),
                "com.amazonaws.dataexchange",
                $wire,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $wire,
            $idx,
        )
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.dataexchange#", $local, "$", $wire),
                "com.amazonaws.dataexchange",
                $wire,
            ),
            dx_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// map writes each key and value against the MAP member's schema, and a list of structures
/// writes each element against the element shape's own `SCHEMA`.
macro_rules! dx_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, boolean) => {
        $s.write_boolean($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, list_of($inner:ty)) => {
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

/// Write a member only when it belongs in the body: a `@required` member always (`req`),
/// a `@default` member always (`dflt` — the vendored codegen writes a non-`Option` field
/// unconditionally), and an `Option` one when it is `Some` (`opt`).
macro_rules! dx_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        dx_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, dflt, $($kind:tt)+) => {
        dx_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            dx_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Element and entry COUNTS are bounded by the request body the server already reads with
/// a cap, and nesting DEPTH by the codec itself, which refuses a body nesting deeper than
/// its own limit.
macro_rules! dx_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, boolean) => { $d.read_boolean($m)? };
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
    ($d:ident, $m:expr, list_of($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__dl)?,
            );
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, a `@default` one takes the type's default, and an optional one
/// stays `None`.
macro_rules! dx_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, dflt) => {
        $slot.unwrap_or_default()
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one output structure, from its
/// member list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output
/// shape, with `@timestampFormat` honoured.
///
/// The `DeserializableStruct` half is reproduced because an output shape nested in a
/// still-derived one (a list entry under `ListDataSetsResponse`, say) is named by the
/// derive's own read code, which requires the bound; the server itself deserializes only
/// `Operation::Input`.
///
/// Each member entry is `(CONST, field, "WireName", index, presence, kind)`. The entries
/// must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! dx_response_shape {
    (
        $ty:ident, $schema_static:ident, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    dx_member_schema!($local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        concat!("com.amazonaws.dataexchange#", $local),
                        "com.amazonaws.dataexchange",
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
                    dx_write_present_member!(
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
                                    dx_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: dx_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
