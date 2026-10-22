Issue-Severity: HIGH
Issue-Gap-Class: fidelity
Issue-Commit: mainline

# `@timestampFormat` never reaches a generated member schema, so JSON timestamps go out as epoch seconds

_Filed: 2026-10-22T00:00:00Z_
_Operation: cross-service (every JSON/CBOR operation with a `date-time` or `http-date` timestamp member) | Class: fidelity_

## Description

`smithy_macros::smithy(shape = "…")` emits a `Schema::new_member(..)` per struct member
and chains the member's wire traits onto it — `.with_xml_name(..)`, `.with_json_name(..)`,
the `@http*` bindings, the constraint `TraitMap`. It never chains the member's
**`@timestampFormat`**: there is no `timestamp_format` / `TimestampFormat` anywhere under
`vendor/smithy-macros/`.

The codecs read the format off the member schema and fall back to their protocol default
when it is absent:

- `vendor/smithy-schema-server/src/json.rs:518` — `write_timestamp` calls
  `timestamp_format(schema.timestamp_format()…)`, defaulting to epoch seconds on
  `restJson1`/`awsJson*`.
- `vendor/smithy-schema-server/src/http_binding.rs:604` and `:1115`, and
  `vendor/smithy-schema-server/src/query.rs:123`, read the same accessor.

So a member the model marks `@timestampFormat("date-time")` is serialized as a NUMBER of
epoch seconds instead of an RFC 3339 string. That is not merely a different rendering of
the same value: it is a different JSON type, and an AWS SDK deserializing a `date-time`
member rejects a number. The reverse case (`@timestampFormat("epoch-seconds")` on a
protocol whose default is `date-time`) is wrong the same way.

## Evidence

`ApiGatewayManagementApi`'s `GetConnection` was the first operation to hit this. Its
`ConnectedAt` / `LastActiveAt` target `__timestampIso8601`, which the model marks
`@timestampFormat("date-time")`
(`smithy_query(query="closure", operation="GetConnection", model_path="services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json")`),
yet the derived schema produced

```json
{"connectedAt":1000000,"identity":{"sourceIp":"192.0.2.10","userAgent":"AWS-SDK-Test/1.0"},"lastActiveAt":1000000}
```

where the real service answers `"connectedAt":"1970-01-12T13:46:40Z"`.

Any service whose model carries an explicit `@timestampFormat` is affected; a quick way
to list candidates is to grep the shipped models for `smithy.api#timestampFormat`.

## What a fix needs

The real fix is in the generator, which is in the agent-immutable `vendor/` tree: in
`vendor/smithy-macros/src/lib.rs`, where the per-member schema is built (the
`member_static_decls.push(..)` near the end of the struct arm), read the member's — and,
failing that, its target shape's — `smithy.api#timestampFormat` trait and chain
`.with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::{DateTime,
EpochSeconds, HttpDate})` onto the emitted `Schema`. `with_timestamp_format` already
exists on `aws_smithy_schema::Schema` and the codecs already read it, so nothing below
the macro has to change.

Until then the only remedy available inside `services/` is per-shape: write that one
shape's `Schema` + `SerializableStruct` out by hand with the modelled format, as
`services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/ops/types/get_connection_types.rs`
does for `GetConnectionResponse`. That does not scale — it is one hand-maintained copy of
generated code per affected shape — which is why this is filed rather than repeated.

## Steps to Reproduce

1. Pick any operation whose output has a timestamp member with an explicit
   `@timestampFormat("date-time")` on a JSON protocol, and whose output struct uses the
   `#[smithy_macros::smithy(shape = …)]` derive.
2. Call it and read the response body.
3. The member is a number (epoch seconds), not the modelled RFC 3339 string.
