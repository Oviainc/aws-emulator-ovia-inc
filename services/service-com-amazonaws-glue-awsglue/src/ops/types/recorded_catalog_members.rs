// SPDX-License-Identifier: CC0-1.0

//! The schema and the serializer a response structure needs when the service reports a
//! member this crate's on-disk Smithy model does not declare.
//!
//! WHY THIS MODULE EXISTS. `#[smithy_macros::smithy(shape = ...)]` derives a structure's
//! schema and codec from the model's member list and admits exactly that list, so a Rust
//! field with no member behind it does not compile. The Glue business-catalog reads
//! (`GetGlossary`, `GetGlossaryTerm`, `ListGlossaryTerms`, `AssociateGlossaryTerms`,
//! `GetAsset`) each answer with one or more members the model omits, so those structures
//! carry their schema and serializer from the macros below instead: one `M_<MEMBER>` member
//! schema per member (index = that member's position in the member list), a `SCHEMA` naming
//! them in that order, the `<SHAPE>_SCHEMA` static and the `SerializableStruct` impl — the
//! same items the derive emits.
//!
//! The model's own members keep the indices and the order the model gives them (the order
//! `smithy_query(query="operation", operation="<Name>")` lists them in), and a member the
//! model does not declare is appended LAST with its shape id in a namespace of its own, so
//! it can neither claim to be modelled nor collide with a member the model may declare
//! later — and a later model refresh only has to move it into place and put the derive
//! back.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reason the derive's own output would be inert
//! here: the constraint traits (the validating walk runs on operation INPUTS only, and
//! every structure built with these macros is output-only), the `@http` chain (baked on
//! only for an operation's input), `@xmlName`/`@jsonName` (no member below declares either:
//! each one's wire name IS its Smithy member name) and `DeserializableStruct` (nothing
//! reads these answers back off the wire).
//!
//! The structures that need them live in this service's per-operation `types/` modules,
//! where each shape is declared exactly once, so `types/mod.rs` pulls this module in with
//! `#[macro_use]` ahead of those modules, which is what puts these macros in their textual
//! scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches `types` through an
//! `include!`, so a `macro_rules!` declared under it counts as macro-expanded and cannot be
//! named by an absolute `$crate::` path (rust-lang #52234).

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! glue_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (timestamp) => {
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
    (map_of_struct) => {
        ::aws_smithy_schema::ShapeType::Map
    };
}

/// One member's schema, as the vendored codegen would emit it.
///
/// `$kind` is the member's shape kind — `string`, `integer`, `timestamp`,
/// `list_of_string`, `list_of_struct(T)`, `map_of_struct(T)` — or
/// `recorded("<namespace>", <kind>)` for a member the model does not declare at all.
macro_rules! glue_member_schema {
    // A member the model does not declare, which the real service carries on the wire: its
    // shape id sits in the namespace given here rather than the service's own. Its
    // encoding is the one its `$kind` names, like any other member.
    (
        $ns:literal, $local:literal, $wire:literal, $idx:literal,
        recorded($rns:literal, $kind:tt $(($inner:ty))?)
    ) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($rns, "#", $local, "$", $wire),
                $rns,
                $wire,
            ),
            glue_shape_type!($kind),
            $wire,
            $idx,
        )
    };
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            glue_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// list of structures writes each element against the element shape's own `SCHEMA`, and a
/// map writes the key and then the value against the member schema.
macro_rules! glue_write_member {
    // A member the model does not declare writes exactly like the kind it wraps; only its
    // shape id differs (see `glue_member_schema!`).
    ($s:ident, $m:expr, $v:expr, recorded($rns:literal, $($kind:tt)+)) => {
        glue_write_member!($s, $m, $v, $($kind)+);
    };
    ($s:ident, $m:expr, $v:expr, string) => {
        $s.write_string($m, $v.as_str())?;
    };
    ($s:ident, $m:expr, $v:expr, timestamp) => {
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
    ($s:ident, $m:expr, $v:expr, list_of_struct($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, map_of_struct($inner:ty)) => {
        $s.write_map($m, &|__sm| {
            for (__k, __v) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_struct($m, __v)?;
            }
            Ok(())
        })?;
    };
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! glue_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        glue_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            glue_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The schema and the serializer for one structure, from its member list: what
/// `#[smithy_macros::smithy(shape = ...)]` emits for an output shape, for a shape that
/// carries a member the model does not declare.
///
/// Each member entry is `(CONST, field, "WireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field) and `opt` an `Option` one. The entries are given
/// in the order the struct declares its fields, which is the model's member order with the
/// unmodelled members appended last.
macro_rules! glue_schema_and_serializer {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    glue_member_schema!($ns, $local, $wire, $idx, $($kind)+);
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
                    glue_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }
    };
}
