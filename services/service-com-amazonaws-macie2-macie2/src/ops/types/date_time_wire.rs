// SPDX-License-Identifier: CC0-1.0

//! Member schemas for the response shapes that carry a `date-time` instant.
//!
//! WHY THIS MODULE EXISTS. Every timestamp member of this service targets
//! `com.amazonaws.macie2#__timestampIso8601`, a `timestamp` shape carrying
//! `@timestampFormat("date-time")`
//! (`smithy_query(query="shape",shape="__timestampIso8601",model_path="services/service-com-amazonaws-macie2-macie2/smithy-model/com-amazonaws-macie2-macie2.json")`
//! reports `"timestampFormat": "date-time"`), and every such member is
//! documented as "in UTC and extended ISO 8601 format". On the restJson1 wire
//! this service speaks, that string form is part of the contract.
//!
//! The JSON codec resolves a timestamp's wire form per member, from the member
//! schema's own format (`vendor/smithy-schema-server/src/json.rs`'s
//! `write_timestamp` -> `Schema::timestamp_format()`), and falls back to the
//! protocol default — epoch seconds, a JSON NUMBER — when the member schema
//! reports none. `vendor/smithy-macros` captures `@length`, `@pattern`,
//! `@jsonName`, `@xmlName` and the HTTP bindings onto each member schema it
//! derives, but never `@timestampFormat`. A derived response shape therefore
//! renders `"createdAt": 1793564017.685` where the service renders an ISO 8601
//! string, which an SDK deserializer bound to a `date-time` member cannot read.
//!
//! `vendor/` is read-only from inside the emulator, so the compensation lives
//! here: the response shapes that carry an instant write their own member
//! schemas (through the two constructors below) and their own
//! `SerializableStruct`/`DeserializableStruct` pair, mirroring the derive
//! exactly — the same member order and `member_index` values, the same shape
//! ids, the same `@jsonName` chain and the same write/read call per member kind
//! — plus the one chain call the derive omits.
//!
//! WHAT IS DELIBERATELY ABSENT, for the same reasons the sibling services'
//! copies of this compensation give:
//!
//! * constraint traits — the validating walk that reads them runs on an
//!   operation's INPUT schema only, and every shape built here is output-only;
//! * the `@http` chain — baked on only when a shape is an operation's input,
//!   and none of these is;
//! * map and list sub-member schemas — only the XML codecs read them, to name
//!   an element, and this service's sole protocol is restJson1.
//!
//! REMOVE THIS MODULE when the scaffold's codegen learns
//! `smithy.api#timestampFormat`: the defect is the codegen's, it affects every
//! `@timestampFormat` member of every service in this workspace, and it is
//! already filed as
//! `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas.md`.
//! `tests/all/macie_session_and_custom_data_identifiers.rs` pins the wire form,
//! so it fails if a response stops carrying it.

/// This service's Smithy namespace, the second segment of every shape id.
const NAMESPACE: &str = "com.amazonaws.macie2";

/// One member schema, exactly as the derive emits it: the shape id, the wire
/// name (every member of this model declares `@jsonName` equal to its own
/// name), the member's shape type and its position in the model's member list.
pub(super) const fn member(
    shape_id: &'static str,
    name: &'static str,
    shape_type: ::aws_smithy_schema::ShapeType,
    index: usize,
) -> ::aws_smithy_schema::Schema {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(shape_id, NAMESPACE, name),
        shape_type,
        name,
        index,
    )
    .with_json_name(name)
}

/// The same, for a member targeting `__timestampIso8601`, with the
/// `@timestampFormat("date-time")` chain call the derive omits. THE reason this
/// module exists.
pub(super) const fn date_time_member(
    shape_id: &'static str,
    name: &'static str,
    index: usize,
) -> ::aws_smithy_schema::Schema {
    member(
        shape_id,
        name,
        ::aws_smithy_schema::ShapeType::Timestamp,
        index,
    )
    .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
}
