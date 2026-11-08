// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`, or when AWS OMITS a member of it the model marks
//! `@required`.
//!
//! WHY THIS MODULE EXISTS. Every `Timestamp` member of this service is the
//! `com.amazonaws.codeguruprofiler#Timestamp` shape, and that shape carries
//! `@timestampFormat("date-time")` —
//! `smithy_query(query="shape",shape="Timestamp,ProfilingGroupDescription,GetRecommendationsResponse")`
//! reports `"timestampFormat": "date-time"` on the shape and on every member targeting it,
//! and each member's own doc says so in prose ("Specify using the ISO 8601 format. For
//! example, 2020-06-01T13:15:02.001Z"). On the restJson1 wire that makes the RFC 3339 /
//! ISO-8601 STRING form part of the contract.
//!
//! The JSON codec decides the rendering per member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch seconds,
//! a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is
//! hand-written service code). So a macro-built `date-time` member schema reports NO
//! format and every such member went onto the wire as a bare epoch number — a type no
//! generated SDK client can deserialize into the modelled `date-time`.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list), a `SCHEMA` naming them in that order, the `<SHAPE>_SCHEMA`
//! static, and the `SerializableStruct`/`DeserializableStruct` pair — plus the one chain
//! call the codegen omits, which [`profiler_member_schema!`] attaches for the `date_time`
//! member kind.
//!
//! THE THIRD REASON: a member of a list can be ABSENT without the list losing its
//! position. `BatchGetFrameMetricData` reports one position per time step and no value at
//! a step it has no metric data for, so that position goes onto the wire as `null` while
//! the list stays index-aligned with the end times it is read beside. The vendored
//! codegen's list writers take a `Vec<T>` and always write a `T`, so the `values` of a
//! `FrameMetricDatum` are a `Vec<Option<f64>>`, written by the `list_of_optional_double`
//! member kind below.
//!
//! THE SECOND REASON a structure lands here: the vendored codegen forces a `@required`
//! member to be a non-`Option` Rust field and ALWAYS writes it, while this service's
//! recorded responses omit some `@required` members outright (`ListProfilingGroups`
//! returns `profilingGroupNames` "only if `includeDescription` is `false`", per that
//! member's own model doc, and `GetRecommendations` omits its two analysis lists rather
//! than returning them empty). Writing the codec by hand is what lets a member be absent
//! from the body; `opt` and `req_unless_empty` below are the two presences that do it.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only, and every structure built with these macros is
//!   output-only, so the macro's captured constraints would be inert here too.
//! * Map/list sub-member schemas (`.with_list_member(..)`). Only the XML codecs read them,
//!   to name an element; this service's sole protocol is restJson1.
//! * `@xmlName`/`@jsonName`/HTTP bindings. No member of the shapes below declares any:
//!   each one's wire name IS its Smithy member name, and the `@httpPayload` members of
//!   this service live on operation output shapes that stay macro-built.
//!
//! REMOVE THE TIMESTAMP HALF OF THIS MODULE when the vendored codegen learns
//! `smithy.api#timestampFormat`: that fix belongs there, because every `@timestampFormat`
//! member of every service in this workspace has the same defect, and this is not the
//! first service to compensate for it by hand (see
//! `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/types/date_time_wire.rs`
//! and the `with_timestamp_format` call sites beside it). Once `vendor/smithy-macros`
//! emits `with_timestamp_format`, put `#[smithy_macros::smithy(shape = ...)]` back on the
//! structures that only needed the format; `tests/all/wire_response_shape.rs` pins the
//! wire form, so it fails if the format does not reach the wire.
//!
//! The structures that need these macros are spread across this service's per-operation
//! `types/` modules, where each shape is declared exactly once, so `types/mod.rs` pulls
//! this module in with `#[macro_use]` — ahead of those modules, which is what puts these
//! macros in their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs`
//! reaches `types` through an `include!`, so a `macro_rules!` declared under it counts as
//! macro-expanded and cannot be named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! profiler_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (double) => {
        ::aws_smithy_schema::ShapeType::Double
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (list_of_optional_double) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (map_of_list_of) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `integer`, `double`, `date_time`, `list_of_string`, `list_of(T)`,
/// `list_of_optional_double`, `map_of_string`, `map_of_list_of(T)` or `struct_of(T)`.
macro_rules! profiler_member_schema {
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
            profiler_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// map writes each value against the MAP member's schema and a list of structures writes
/// each element against the element shape's own `SCHEMA`.
macro_rules! profiler_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, integer) => { $s.write_integer($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, double) => { $s.write_double($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, list_of_optional_double) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                match __it {
                    ::std::option::Option::Some(__number) => {
                        __sl.write_double($m, *__number)?;
                    }
                    // The position is kept and carries no value, which is what keeps the
                    // list aligned with whatever it is read beside.
                    ::std::option::Option::None => __sl.write_null($m)?,
                }
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
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __mv.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, map_of_list_of($inner:ty)) => {
        $s.write_map($m, &|__sm| {
            for (__k, __mv) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_list($m, &|__sl| {
                    for __it in __mv.iter() {
                        __sl.write_struct(&<$inner>::SCHEMA, __it)?;
                    }
                    Ok(())
                })?;
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
/// depth guard at the entry of `read_struct`, `read_list` and `read_map` and refuses a body
/// nesting deeper than its `MAX_DEPTH`, which the protocol boundary renders as a client
/// refusal.
macro_rules! profiler_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, integer) => { $d.read_integer($m)? };
    ($d:ident, $m:expr, double) => { $d.read_double($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, list_of_optional_double) => {{
        let mut __out: ::std::vec::Vec<::std::option::Option<f64>> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            if __dl.is_null() {
                __dl.read_null()?;
                __out.push(::std::option::Option::None);
            } else {
                __out.push(::std::option::Option::Some(__dl.read_double($m)?));
            }
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
    ($d:ident, $m:expr, map_of_list_of($inner:ty)) => {{
        let mut __out: ::std::collections::HashMap<
            ::std::string::String,
            ::std::vec::Vec<$inner>,
        > = ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            let mut __list: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
            __dm.read_list($m, &mut |__dl| {
                __list.push(
                    <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(
                        __dl,
                    )?,
                );
                Ok(())
            })?;
            __out.insert(__k, __list);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when it belongs in the body: a `@required` member always (`req`),
/// an `Option` one when it is `Some` (`opt`), and a `@required` COLLECTION the service
/// omits rather than reports empty only when it holds something (`req_unless_empty`).
macro_rules! profiler_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        profiler_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            profiler_write_member!($s, &$m, __v, $($kind)+);
        }
    };
    ($s:ident, $this:ident, $m:path, $field:ident, req_unless_empty, $($kind:tt)+) => {
        if !$this.$field.is_empty() {
            profiler_write_member!($s, &$m, (&$this.$field), $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`, and a collection the service
/// omits when it is empty reads back as the empty collection.
macro_rules! profiler_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
    ($slot:ident, $wire:literal, req_unless_empty) => {
        $slot.unwrap_or_default()
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape,
/// with `@timestampFormat` honoured and with a member the service omits able to be absent.
///
/// Each member entry is `(CONST, field, "wireName", index, presence, kind)`. The entries
/// must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! profiler_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    profiler_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    profiler_write_present_member!(
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
                                    profiler_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: profiler_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
