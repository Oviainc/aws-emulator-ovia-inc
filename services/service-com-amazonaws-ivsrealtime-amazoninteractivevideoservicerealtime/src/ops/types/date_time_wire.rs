// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec one structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets a `timestamp`
//! shape carrying `@timestampFormat("date-time")` — `com.amazonaws.ivsrealtime#Time` for
//! the composition, destination, stage-session, participant and event times, and
//! `ParticipantTokenExpirationTime` for the two token expiries
//! (`smithy_query(query="shape",shape="Composition,Destination,StageSession,Participant,Event,ExchangedParticipantToken,ParticipantToken")`
//! reports `"timestampFormat": "date-time"` on each one, and each member's own doc says
//! the value is "returned as a string"). On the restJson1 wire this service speaks, that
//! makes the RFC 3339 / ISO-8601 STRING form part of the contract. The JSON codec decides
//! the rendering per member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds,
//! a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is hand-written
//! service code). So a macro-built `date-time` member schema reports NO format and every
//! such member went onto the wire as a bare epoch number — not even the modelled JSON
//! type, so an SDK whose generated deserializer expects a `date-time` string cannot read
//! it.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA`
//! static and the `SerializableStruct`/`DeserializableStruct` pair — plus the one chain
//! call the codegen omits, which [`ivsrt_member_schema!`] attaches for the `date_time`
//! member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only, and every structure built with these macros is
//!   output-only — no operation of this service takes a `Composition`, `Destination`,
//!   `CompositionSummary`, `DestinationSummary`, `StageSession`, `StageSessionSummary`,
//!   `Participant`, `ParticipantSummary`, `Event`, `ExchangedParticipantToken` or a
//!   `ParticipantToken` as its input — so the derive's captured constraints would be inert
//!   on them too.
//! * The `@http` chain. It is baked onto a shape only when the shape is an operation's
//!   INPUT; none of these is.
//! * Map/list sub-member schemas (`.with_map_members(..)` / `.with_list_member(..)`). Only
//!   the XML and query codecs read them, to name an element; this service's sole protocol
//!   is restJson1.
//! * `@xmlName`/`@jsonName`. No member of the shapes below declares either: each one's
//!   wire name IS its Smithy member name.
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand — `services/service-com-amazonaws-emrcontainers-awschicagowebservice/\
//! src/ops/types/date_time_wire.rs`,
//! `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/types/\
//! date_time_wire.rs` and
//! `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/types/\
//! date_time_wire.rs` carry near-identical macros. Once `vendor/smithy-macros` emits
//! `with_timestamp_format`, delete these macros and the `ivsrt_schema_and_codec!` blocks
//! and put `#[smithy_macros::smithy(shape = ...)]` back on the structures that carry one —
//! and delete those sibling copies in the same change.
//! `tests/all/date_time_members_wire_form.rs` pins the wire form, so it fails if the
//! format does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types`
//! through an `include!`, so a `macro_rules!` declared under it counts as macro-expanded
//! and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! ivsrt_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
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
    (list_of_struct) => {
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
/// string-encoded), `boolean`, `integer`, `date_time`, `map_of_string`, `list_of_string`,
/// `list_of_struct(T)` or `struct_of(T)`.
macro_rules! ivsrt_member_schema {
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
            ivsrt_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// map writes each key and value, and a list of scalars each element, against the MAP or
/// LIST member's own schema, while a list of structures writes each element against the
/// ELEMENT shape's own `SCHEMA`.
macro_rules! ivsrt_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, boolean) => { $s.write_boolean($m, *$v)?; };
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
    ($s:ident, $m:expr, $v:expr, list_of_struct($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
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
macro_rules! ivsrt_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, boolean) => { $d.read_boolean($m)? };
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
    ($d:ident, $m:expr, list_of_struct($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        // Count the element being read so a required member absent from one of them
        // reports its position, as the vendored codegen does for a list whose element can
        // carry one.
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
}

/// Write a member only when the value is there: an `Option` one when it is `Some`, a
/// non-`Option` one (`@required`, or `@default`) always — the vendored codegen's own
/// `is_option` split.
macro_rules! ivsrt_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        ivsrt_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, def_false, $($kind:tt)+) => {
        ivsrt_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            ivsrt_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`, and a `@default` one takes the
/// default the model declares for it — `def_false` for every such member of this service,
/// each of which declares `@default false`.
macro_rules! ivsrt_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, def_false) => {
        $slot.unwrap_or(false)
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape,
/// with `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "wireName", index, req|def_false|opt, kind)`;
/// `req` is a `@required` member (a bare Rust field, refused when absent on the wire),
/// `def_false` a `@default false` one (a bare field that falls back to that declared
/// default) and `opt` an `Option` one. The entries must be given in the model's member
/// order, which is the order `smithy_query(query="shape", shape=...)` lists them in and
/// the order each struct declares its fields in.
macro_rules! ivsrt_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    ivsrt_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    ivsrt_write_present_member!(
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
                                    ivsrt_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: ivsrt_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
