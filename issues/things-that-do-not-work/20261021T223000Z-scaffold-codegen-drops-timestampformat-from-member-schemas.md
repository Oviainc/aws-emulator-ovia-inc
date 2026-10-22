Issue-Severity: HIGH
Issue-Gap-Class: wrong

# `@timestampFormat` is dropped from member schemas, so every `date-time` timestamp renders as epoch seconds

_Filed: 2026-10-21T22:30:00Z_
_Operation: scaffold-codegen-timestamp-format | Class: wrong_

## Description

`#[smithy_macros::smithy(shape = "…")]` captures a member's `@length`, `@range`,
`@pattern`, enum value set, `@uniqueItems`, `@jsonName`, `@xmlName` and HTTP bindings
onto the member's `aws_smithy_schema::Schema`, but it never captures
`@timestampFormat` — neither the member's own trait nor the one on the timestamp
shape the member targets.

Every codec resolves the format from that member schema:

- `vendor/smithy-schema-server/src/json.rs::write_timestamp` →
  `timestamp_format(schema.timestamp_format())`, which maps `None` to
  `Format::EpochSeconds`;
- `vendor/smithy-schema-server/src/http_binding.rs` and
  `vendor/smithy-schema-server/src/query.rs` resolve it the same way.

So on the JSON protocols (`restJson1`, `awsJson1_0`, `awsJson1_1`) a member that the
model declares `@timestampFormat("date-time")` is written as an epoch-seconds NUMBER
instead of an ISO-8601 STRING. An SDK generated from the same model expects a string
there and fails to deserialize the response.

## Evidence

- `grep -rn 'timestamp_format\|timestampFormat' vendor/smithy-macros/` → no matches:
  the derive has no code path that reads the trait. `vendor/smithy-macros/src/lib.rs`
  builds each member as
  `Schema::new_member(..) #xml_name_chain #json_name_chain #http_binding_chain #collection_members_chain #traits_chain`
  — no timestamp-format link in the chain.
- `aws_smithy_schema::Schema::with_timestamp_format(traits::TimestampFormat::DateTime)`
  exists and is `const`, so the derive can emit it; nothing in the tree calls it except
  the hand-written workaround below.
- Concrete instance:
  `services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice`.
  `smithy_query(query="shape", shape="MediaCapturePipeline")` reports
  `"timestampFormat": "date-time"` on `CreatedTimestamp` and `UpdatedTimestamp` (both
  target `Iso8601Timestamp`, which carries the trait). With the derived schema,
  `CreateMediaCapturePipeline` answered
  `"CreatedTimestamp": 1761081534.919`; the service answers
  `"CreatedTimestamp": "2026-10-21T21:18:54.919Z"`.

## Current workaround, and why it should be removed

`services/service-com-amazonaws-chimesdkmediapipelines-chimesdkmediapipelinesservice/src/ops/types/shared_types.rs`
hand-writes `MediaCapturePipeline`'s member schemas, `SerializableStruct` and
`DeserializableStruct` so the two timestamp members can carry
`.with_timestamp_format(TimestampFormat::DateTime)`. That duplicates ~200 lines of
derive output, and pins the member order, `member_index` values and shape ids by
hand, so it can silently drift from the model. Every other service with a `date-time`
member will copy the same block unless the derive is fixed.

## Steps to Reproduce

1. Pick any operation whose output carries a member targeting a timestamp shape with
   `@timestampFormat("date-time")` on a JSON protocol — e.g.
   `GetMediaCapturePipeline`'s `MediaCapturePipeline$CreatedTimestamp`.
2. Let the member's schema come from `#[smithy_macros::smithy(shape = "…")]`.
3. Call the operation and read the body: the member is a JSON number of epoch
   seconds, where the model (and therefore every generated SDK) requires an ISO-8601
   string.

## Required change

In `vendor/smithy-macros` (agent-immutable from a service crate, so this needs a
core-library change), resolve `@timestampFormat` for every member — the member's own
trait first, then the trait on the shape it targets — and emit
`.with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::{DateTime,
HttpDate, EpochSeconds})` onto that member's schema, the way `@jsonName` and the HTTP
bindings are already emitted. Then delete the hand-written `MediaCapturePipeline`
block named above and let the derive produce it again.
