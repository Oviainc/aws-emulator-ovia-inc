// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.bedrockagentcorecontrol#DateTimestamp`, which the model marks
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="DateTimestamp",model_path="services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/smithy-model/com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json")`),
//! so on this service's restJson1 wire every one of them is an RFC 3339 / ISO-8601 STRING.
//! The JSON codec decides the rendering per member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs`: `write_timestamp` reads
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds, a
//! JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is hand-written
//! service code). So a macro-built `date-time` member schema reports NO format and every
//! such member went onto the wire as a bare epoch number — not even the modelled JSON type,
//! so an SDK deserializing the member rejects or mis-parses it.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros below
//! emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]` emits —
//! one `M_<MEMBER>` member schema per member (index = that member's position in the model's
//! member list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA` static and the
//! `SerializableStruct`/`DeserializableStruct` pair — plus the one chain call the codegen
//! omits, which [`agentcore_member_schema!`] attaches for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only (`aws_smithy_schema_server::validate_to_message`, which
//!   the generated service calls once on the deserialized input before the handler runs).
//!   Every structure built with these macros is output-only, so the derive's captured
//!   constraints would be inert on them too.
//! * Map/list sub-member schemas (`.with_list_member(..)`). Only the XML codecs read them,
//!   to name an element; this service's sole protocol is restJson1.
//! * `@xmlName`/`@jsonName`. No member of the shapes below declares either: each one's wire
//!   name IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to compensate for it by
//! hand — `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/types/date_time_wire.rs`
//! and `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/types/date_time_wire.rs`
//! carry near-identical macros for the same reason. Once `vendor/smithy-macros` emits
//! `with_timestamp_format`, delete these macros and the `agentcore_schema_and_codec!`
//! blocks and put `#[smithy_macros::smithy(shape = ...)]` back on the structures that carry
//! one. `tests/all/gateway_wire_shape.rs` pins the wire form, so it fails if the format
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
macro_rules! agentcore_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of_struct) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of_union) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
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
/// string-encoded), `integer`, `date_time`, `list_of_string`, `map_of_string`,
/// `list_of_struct(T)`, `list_of_union(T)`, `struct_of(T)` or `union_of(T)`.
macro_rules! agentcore_member_schema {
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
            agentcore_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// list of structures (or unions) writes each element against the element shape's own
/// `SCHEMA`, and a map writes the key and the value against the member schema.
macro_rules! agentcore_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, integer) => {
        $s.write_integer($m, *$v)?;
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
    ($s:ident, $m:expr, $v:expr, list_of_union($inner:ty)) => {
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
    ($s:ident, $m:expr, $v:expr, union_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Element counts are bounded by the request body the server already reads with a cap, and
/// nesting DEPTH by the codec itself: the vendored JSON deserializer takes a depth guard at
/// the entry of `read_struct`, `read_list` and `read_map` and refuses a body nesting deeper
/// than its `MAX_DEPTH`, which the protocol boundary renders as a client refusal.
macro_rules! agentcore_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, integer) => { $d.read_integer($m)? };
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<::std::string::String, ::std::string::String> =
            ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            let __entry = __dm.read_string($m)?;
            __out.insert(__k, __entry);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, list_of_struct($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        let mut __pos = 0usize;
        $d.read_list($m, &mut |__dl| {
            __pos += 1;
            __out.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__dl)?,
            );
            Ok(())
        })
        .map_err(|__e| ::aws_smithy_schema_server::within_list_element(__e, __pos))?;
        __out
    }};
    ($d:ident, $m:expr, list_of_union($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        let mut __pos = 0usize;
        $d.read_list($m, &mut |__dl| {
            __pos += 1;
            __out.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__dl)?,
            );
            Ok(())
        })
        .map_err(|__e| ::aws_smithy_schema_server::within_list_element(__e, __pos))?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
    ($d:ident, $m:expr, union_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! agentcore_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        agentcore_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            agentcore_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! agentcore_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member list:
/// everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape, with
/// `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "wireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct declares
/// its fields in.
macro_rules! agentcore_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    agentcore_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    agentcore_write_present_member!(
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
                                    agentcore_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: agentcore_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
