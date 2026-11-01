// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec for every structure of this service that carries a
//! `@timestampFormat("date-time")` member.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service's model carries
//! `@timestampFormat("date-time")` and is documented as "The ISO-8601 timestamp …"
//! (`smithy_query(query="shape", shape="CrlDetail,TrustAnchorDetail,ProfileDetail,SubjectDetail,SubjectSummary,CredentialSummary,InstanceProperty",
//! model_path="services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json")`
//! reports `"timestampFormat": "date-time"` on each of them). On the restJson1 wire this
//! service speaks, that makes the RFC 3339 / ISO-8601 STRING form part of the contract.
//! The JSON codec decides the rendering per member from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs`: `write_timestamp` reads
//! `Schema::timestamp_format()`) and falls back to the protocol default — epoch seconds,
//! a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@jsonName`, `@xmlName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)`, so a macro-built timestamp member reports no
//! format and went onto the wire as a bare epoch number — not the modelled JSON type, so
//! an SDK deserializing it either fails or surfaces a wrong instant. `vendor/` is
//! read-only from inside the emulator, so the structures that carry such a member declare
//! their schema and codec here instead, with the one chain call the codegen omits.
//!
//! The macros below emit, for one structure, exactly what
//! `#[smithy_macros::smithy(shape = ...)]` emits for a response shape: one `M_<MEMBER>`
//! member schema per member (index = that member's position in the model's member list,
//! name as the model spells it), a `SCHEMA` naming them in that order, the
//! `<SHAPE>_SCHEMA` static and the `SerializableStruct` / `DeserializableStruct` pair.
//! Each member list is the model's, member for member: same shape ids, same wire names,
//! same order, same shape types. The ONE exception is marked where it is declared:
//! `TrustAnchorDetail$crlDpRevocationEnabled`, which the vendored model snapshot does not
//! carry while the recorded service answers with it.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@range`, `@pattern`). The validating walk that
//!   enforces them runs on operation INPUTS only
//!   (`aws_smithy_schema_server::validate_to_message`, called once on the deserialized
//!   input before the handler runs), and every structure below is a response shape or a
//!   list element of one.
//! * The `@http` chain. The vendored codegen bakes method and URI onto a shape only when
//!   the shape is an operation's INPUT; none of these is an input.
//! * `.with_list_member(..)` / `.with_map_members(..)`. Only the XML codecs read them, to
//!   name an element; this service's sole protocol is restJson1.
//! * `@jsonName` / `@xmlName`. No member of this service's model declares either, so each
//!   member's wire name IS its Smithy member name.
//!
//! A `DeserializableStruct` impl IS emitted: each of these structures is nested inside a
//! response shape that keeps the derive, and the derive's generated deserializer names the
//! nested type's `deserialize`, so the impl has to exist for the crate to compile. Nothing
//! in the emulator reads a response back, and element and entry COUNTS on that path are
//! bounded by the request body the server already reads with a cap, nesting DEPTH by the
//! codec itself (the vendored JSON deserializer takes a depth guard at the entry of
//! `read_struct`, `read_list` and `read_map`).
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect and several sibling crates compensate for it by hand in
//! their own `src/ops/types/date_time_wire.rs`. Once the codegen emits
//! `with_timestamp_format`, delete these macros and the `ra_schema_and_codec!` blocks and
//! put `#[smithy_macros::smithy(shape = ...)]` back on the structures below.
//! `tests/all/iso8601_timestamp_members.rs` pins the wire form, so it fails if the format
//! does not reach the wire.

use super::{
    AttributeMapping, CredentialSummary, CrlDetail, InstanceProperty, NotificationSettingDetail,
    ProfileDetail, Source, SubjectDetail, SubjectSummary, TrustAnchorDetail,
};

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident` — an `enum` shape is `String`, as that function has it.
macro_rules! ra_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (blob) => {
        ::aws_smithy_schema::ShapeType::Blob
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
/// it and `$idx` its position in the model's member list. `$kind` is the member's shape
/// kind — `string` (a `string` or an `enum` shape, both string-encoded), `boolean`,
/// `integer`, `blob`, `date_time`, `list_of_string`, `list_of_struct(T)`, `map_of_string`
/// or `struct_of(T)`.
macro_rules! ra_member_schema {
    ($local:literal, $member:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.rolesanywhere#", $local, "$", $member),
                "com.amazonaws.rolesanywhere",
                $member,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $member,
            $idx,
        )
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    ($local:literal, $member:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!("com.amazonaws.rolesanywhere#", $local, "$", $member),
                "com.amazonaws.rolesanywhere",
                $member,
            ),
            ra_shape_type!($kind),
            $member,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind.
macro_rules! ra_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, boolean) => { $s.write_boolean($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, integer) => { $s.write_integer($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, blob) => { $s.write_blob($m, &$v[..])?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    // A list of structures writes each element through the element shape's OWN schema,
    // as the vendored codegen's `emit_write` does for a structure element.
    ($s:ident, $m:expr, $v:expr, list_of_struct($inner:ty)) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    // A map of strings writes each entry as the vendored codegen does for
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
macro_rules! ra_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, boolean) => { $d.read_boolean($m)? };
    ($d:ident, $m:expr, integer) => { $d.read_integer($m)? };
    ($d:ident, $m:expr, blob) => { $d.read_blob($m)?.into_inner() };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    // A failing element of a list of structures is named by its position, as the vendored
    // codegen names it for an element that can report an absent `@required` member.
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
macro_rules! ra_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        ra_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            ra_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! ra_member_value {
    ($slot:ident, $member:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($member))?
    };
    ($slot:ident, $member:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list.
///
/// Each member entry is `(CONST, field, "Member", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries are given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! ra_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $local:literal,
        $( (
            $cname:ident, $field:ident, $member:literal, $idx:literal,
            $presence:tt, $($kind:tt)+
        ) ),+ $(,)?
    ) => {
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    ra_member_schema!($local, $member, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static(
                        concat!("com.amazonaws.rolesanywhere#", $local),
                        "com.amazonaws.rolesanywhere",
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
                    ra_write_present_member!(
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
                                    ra_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: ra_member_value!($field, $member, $presence) ),+
                })
            }
        }
    };
}

ra_schema_and_codec!(
    CrlDetail,
    CRL_DETAIL_SCHEMA,
    "CrlDetail",
    (M_CRL_ID, crl_id, "crlId", 0, opt, string),
    (M_CRL_ARN, crl_arn, "crlArn", 1, opt, string),
    (M_NAME, name, "name", 2, opt, string),
    (M_ENABLED, enabled, "enabled", 3, opt, boolean),
    (M_CRL_DATA, crl_data, "crlData", 4, opt, blob),
    (M_TRUST_ANCHOR_ARN, trust_anchor_arn, "trustAnchorArn", 5, opt, string),
    (M_CREATED_AT, created_at, "createdAt", 6, opt, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 7, opt, date_time),
);

ra_schema_and_codec!(
    TrustAnchorDetail,
    TRUST_ANCHOR_DETAIL_SCHEMA,
    "TrustAnchorDetail",
    (M_TRUST_ANCHOR_ID, trust_anchor_id, "trustAnchorId", 0, opt, string),
    (M_TRUST_ANCHOR_ARN, trust_anchor_arn, "trustAnchorArn", 1, opt, string),
    (M_NAME, name, "name", 2, opt, string),
    (M_SOURCE, source, "source", 3, opt, struct_of(Source)),
    (M_ENABLED, enabled, "enabled", 4, opt, boolean),
    (M_CREATED_AT, created_at, "createdAt", 5, opt, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 6, opt, date_time),
    (
        M_NOTIFICATION_SETTINGS,
        notification_settings,
        "notificationSettings",
        7,
        opt,
        list_of_struct(NotificationSettingDetail)
    ),
    // Not in the vendored model snapshot, but on every recorded `TrustAnchorDetail` the
    // real service answers with; see the member's own note on the struct.
    (
        M_CRL_DP_REVOCATION_ENABLED,
        crl_dp_revocation_enabled,
        "crlDpRevocationEnabled",
        8,
        opt,
        boolean
    ),
);

ra_schema_and_codec!(
    ProfileDetail,
    PROFILE_DETAIL_SCHEMA,
    "ProfileDetail",
    (M_PROFILE_ID, profile_id, "profileId", 0, opt, string),
    (M_PROFILE_ARN, profile_arn, "profileArn", 1, opt, string),
    (M_NAME, name, "name", 2, opt, string),
    (
        M_REQUIRE_INSTANCE_PROPERTIES,
        require_instance_properties,
        "requireInstanceProperties",
        3,
        opt,
        boolean
    ),
    (M_ENABLED, enabled, "enabled", 4, opt, boolean),
    (M_CREATED_BY, created_by, "createdBy", 5, opt, string),
    (M_SESSION_POLICY, session_policy, "sessionPolicy", 6, opt, string),
    (M_ROLE_ARNS, role_arns, "roleArns", 7, opt, list_of_string),
    (
        M_MANAGED_POLICY_ARNS,
        managed_policy_arns,
        "managedPolicyArns",
        8,
        opt,
        list_of_string
    ),
    (M_CREATED_AT, created_at, "createdAt", 9, opt, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 10, opt, date_time),
    (M_DURATION_SECONDS, duration_seconds, "durationSeconds", 11, opt, integer),
    (
        M_ACCEPT_ROLE_SESSION_NAME,
        accept_role_session_name,
        "acceptRoleSessionName",
        12,
        opt,
        boolean
    ),
    (
        M_ATTRIBUTE_MAPPINGS,
        attribute_mappings,
        "attributeMappings",
        13,
        opt,
        list_of_struct(AttributeMapping)
    ),
);

ra_schema_and_codec!(
    SubjectDetail,
    SUBJECT_DETAIL_SCHEMA,
    "SubjectDetail",
    (M_SUBJECT_ARN, subject_arn, "subjectArn", 0, opt, string),
    (M_SUBJECT_ID, subject_id, "subjectId", 1, opt, string),
    (M_ENABLED, enabled, "enabled", 2, opt, boolean),
    (M_X509_SUBJECT, x509_subject, "x509Subject", 3, opt, string),
    (M_LAST_SEEN_AT, last_seen_at, "lastSeenAt", 4, opt, date_time),
    (M_CREATED_AT, created_at, "createdAt", 5, opt, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 6, opt, date_time),
    (
        M_CREDENTIALS,
        credentials,
        "credentials",
        7,
        opt,
        list_of_struct(CredentialSummary)
    ),
    (
        M_INSTANCE_PROPERTIES,
        instance_properties,
        "instanceProperties",
        8,
        opt,
        list_of_struct(InstanceProperty)
    ),
);

ra_schema_and_codec!(
    SubjectSummary,
    SUBJECT_SUMMARY_SCHEMA,
    "SubjectSummary",
    (M_SUBJECT_ARN, subject_arn, "subjectArn", 0, opt, string),
    (M_SUBJECT_ID, subject_id, "subjectId", 1, opt, string),
    (M_ENABLED, enabled, "enabled", 2, opt, boolean),
    (M_X509_SUBJECT, x509_subject, "x509Subject", 3, opt, string),
    (M_LAST_SEEN_AT, last_seen_at, "lastSeenAt", 4, opt, date_time),
    (M_CREATED_AT, created_at, "createdAt", 5, opt, date_time),
    (M_UPDATED_AT, updated_at, "updatedAt", 6, opt, date_time),
);

ra_schema_and_codec!(
    CredentialSummary,
    CREDENTIAL_SUMMARY_SCHEMA,
    "CredentialSummary",
    (M_SEEN_AT, seen_at, "seenAt", 0, opt, date_time),
    (M_SERIAL_NUMBER, serial_number, "serialNumber", 1, opt, string),
    (M_ISSUER, issuer, "issuer", 2, opt, string),
    (M_ENABLED, enabled, "enabled", 3, opt, boolean),
    (
        M_X509_CERTIFICATE_DATA,
        x509_certificate_data,
        "x509CertificateData",
        4,
        opt,
        string
    ),
    (M_FAILED, failed, "failed", 5, opt, boolean),
);

ra_schema_and_codec!(
    InstanceProperty,
    INSTANCE_PROPERTY_SCHEMA,
    "InstanceProperty",
    (M_SEEN_AT, seen_at, "seenAt", 0, opt, date_time),
    (M_PROPERTIES, properties, "properties", 1, opt, map_of_string),
    (M_FAILED, failed, "failed", 2, opt, boolean),
);
