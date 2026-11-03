// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.securityhub#Timestamp`, which the model marks
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="Timestamp",model_path="services/service-com-amazonaws-securityhub-securityhubapiservice/smithy-model/com-amazonaws-securityhub-securityhubapiservice.json")`),
//! so on this service's restJson1 wire every one of them is an RFC 3339 / ISO-8601
//! STRING. The JSON codec decides that rendering per MEMBER, from the member schema's
//! own format (`vendor/smithy-schema-server/src/json.rs`: the timestamp writer reads
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch
//! seconds, a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)`. So a macro-built `date-time` member schema
//! reports NO format and every such member went onto the wire as a bare epoch number —
//! not even the modelled JSON type, so an SDK deserializing the member rejects it and the
//! whole response is unusable from a typed client.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one OUTPUT structure, what `#[smithy_macros::smithy(shape = ...)]`
//! emits for it — one `M_<MEMBER>` member schema per member (index = that member's
//! position in the model's member list), a `SCHEMA` naming them in that order, the
//! `<SHAPE>_SCHEMA` static and the `SerializableStruct`/`DeserializableStruct` pair —
//! plus the one chain call the codegen omits, which [`securityhub_member_schema!`]
//! attaches for the `date_time` member kind, and minus what the note below lists.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, `@range`, …). The validating walk that
//!   enforces them runs on operation INPUTS only (the generated service calls it once on
//!   the deserialized input before the handler runs). Every structure built with these
//!   macros is output-only, so the derive's captured constraints would be inert on them
//!   too. An INPUT shape that carries a `date-time` member therefore keeps its schema
//!   written out in full beside its own operation's types (see
//!   `get_finding_history_types.rs`), constraints included, rather than coming through
//!   here.
//! * Map/list sub-member schemas (`.with_list_member(..)`). Only the XML codecs read
//!   them, to name an element; this service's sole protocol is restJson1.
//! * The error trail the derive's read builds — the member name and the list position it
//!   prepends to a failure raised under a nested shape. That trail exists so a REFUSED
//!   request can name the member it was refused for, and the validating read runs on
//!   operation inputs only; every structure built with these macros is output-only, so a
//!   failure inside one is not something a caller can provoke. A shape that needs the
//!   trail keeps its schema written out in full beside its own operation's types.
//! * `@xmlName`/`@jsonName`. This model declares neither on any member: every member's
//!   wire name IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect. The generator gap is filed as
//! `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md`.
//! Once the codegen emits `with_timestamp_format`, delete these macros and the
//! `securityhub_schema_and_codec!` blocks and put `#[smithy_macros::smithy(shape = ...)]`
//! back on the structures that use one. `tests/all/timestamp_wire_form.rs` pins the wire
//! form, so it fails if the format does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types`
//! through an `include!`, so a `macro_rules!` declared under it counts as macro-expanded
//! and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! securityhub_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
    (list_of) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of_strings) => {
        ::aws_smithy_schema::ShapeType::List
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `integer`, `boolean`, `date_time`, `struct_of(T)`, `list_of(T)`
/// for a list whose element is the structure `T`, or `list_of_strings` for a list of
/// strings.
macro_rules! securityhub_member_schema {
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
            securityhub_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind.
macro_rules! securityhub_write_member {
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, integer) => {
        $s.write_integer($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, boolean) => {
        $s.write_boolean($m, *$v)?;
    };
    ($s:ident, $m:expr, $v:expr, date_time) => {
        $s.write_timestamp($m, $v)?;
    };
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => {
        $s.write_struct($m, $v)?;
    };
    // A list of structures writes each element against the ELEMENT shape's own schema,
    // exactly as the codegen does.
    ($s:ident, $m:expr, $v:expr, list_of($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    // A list of strings writes each element against the MEMBER's schema, which is what
    // the codegen does for a scalar element: the JSON codec needs no element schema.
    ($s:ident, $m:expr, $v:expr, list_of_strings) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Nesting DEPTH is bounded by the codec itself: the vendored JSON deserializer takes a
/// depth guard at the entry of its struct, list and map reads and refuses a body nesting
/// deeper than its maximum, which the protocol boundary renders as a client refusal.
macro_rules! securityhub_read_member {
    ($d:ident, $m:expr, string) => {
        $d.read_string($m)?
    };
    ($d:ident, $m:expr, integer) => {
        $d.read_integer($m)?
    };
    ($d:ident, $m:expr, boolean) => {
        $d.read_boolean($m)?
    };
    ($d:ident, $m:expr, date_time) => {
        $d.read_timestamp($m)?
    };
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
    ($d:ident, $m:expr, list_of($inner:ty)) => {{
        let mut __items: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__el| {
            __items.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__el)?,
            );
            Ok(())
        })?;
        __items
    }};
    ($d:ident, $m:expr, list_of_strings) => {{
        let mut __items: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__el| {
            __items.push(__el.read_string($m)?);
            Ok(())
        })?;
        __items
    }};
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! securityhub_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        securityhub_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            securityhub_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! securityhub_member_value {
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
/// Each member entry is `(CONST, field, "WireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct declares
/// its fields in.
macro_rules! securityhub_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    securityhub_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    securityhub_write_present_member!(
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
                                    securityhub_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: securityhub_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
