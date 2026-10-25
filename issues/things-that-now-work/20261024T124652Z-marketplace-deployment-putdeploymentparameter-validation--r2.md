Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# expirationDate is only accepted as an epoch-seconds number; the modelled @timestampFormat("date-time") RFC 3339 string an SDK sends is refused 400

_Filed: 2026-10-24T12:46:52Z_
_Operation: PutDeploymentParameter | Class: validation_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutDeploymentParameterRequest$expirationDate` is modelled `@timestampFormat("date-time")` (smithy_query(query="operation",operation="PutDeploymentParameter") reports `"expirationDate": {"target": "Timestamp", "timestampFormat": "date-time"}`), so on restJson1 the member travels as an RFC 3339 STRING — `"expirationDate": "2100-01-01T00:00:00Z"` — and that is exactly what every AWS SDK serializes for it.

The emulator's input schema for this shape is produced by the plain `#[smithy_macros::smithy(shape = "…PutDeploymentParameterRequest")]` derive (src/ops/types/put_deployment_parameter_types.rs), and that generator never chains `.with_timestamp_format(..)`. With no format on the member schema the restJson1 reader falls back to the protocol default, epoch seconds (vendor/smithy-schema-server/src/json.rs::read_timestamp -> `timestamp_format(schema.timestamp_format())`). Consequences, both observable on the wire:

1. EXPECTED: `{"agreementId":…,"deploymentParameter":{…},"expirationDate":"2100-01-01T00:00:00Z"}` is accepted, the deadline is stored, and the response is 200 with the minted `resourceArn`/`deploymentParameterId`. ACTUAL: the string token is parsed with `Format::EpochSeconds`, `DateTime::from_str` fails, and the request is answered as a framework deserialization failure (HTTP 400, `"invalid timestamp"`) — the parameter is never written at all. Any SDK caller that sets an expiration date cannot create a deployment parameter.
2. Conversely the emulator ACCEPTS a form the modelled member does not admit (`"expirationDate": 4102444800`, a JSON number), which the real service refuses, so a client that works against the emulator is writing a request AWS rejects.

Secondary symptom of the same input-judgement class on this operation: the modelled `TagsMap` key/value constraints (`TagKey`/`TagValue`, length 1..128 / 1..256 and `^[a-zA-Z0-9/_+=.:@-]+$`) are enforced only on the CREATE path. On the UPDATE path (`by_name` already holds the identity) the handler returns before `merge_tags` is ever called, so `tags: {"bad key!": "v"}` against an existing deployment parameter answers 200 where AWS — which validates the modelled input before the handler regardless of whether `tags` is then applied — answers 400 ValidationException.

## Evidence

Spec: `expirationDate` carries `smithy.api#timestampFormat = "date-time"` on the member (smithy_query query="operation", operation="PutDeploymentParameter"); `tags` targets `TagsMap` whose key/value are the constrained `TagKey`/`TagValue` (smithy_query query="shape", shape="TagsMap").

Code: src/ops/types/put_deployment_parameter_types.rs line 22 declares `pub expiration_date: Option<::aws_smithy_types::DateTime>` under the plain derive, and the derive's member schemas never chain `.with_timestamp_format(..)` (grep: every `with_timestamp_format` call site in the tree is a HAND-WRITTEN service schema, e.g. services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/ops/types/get_connection_types.rs:64 and services/service-com-amazonaws-accessanalyzer-accessanalyzer/src/ops/types/shared_types.rs:454, which exist precisely to work around this). Reader: vendor/smithy-schema-server/src/json.rs:1009-1025 parses a string timestamp with the format read off the schema, defaulting to epoch seconds.

Tests: the gap is documented and then encoded by the suite rather than asserted against — tests/all/deployment_parameter_expiry.rs lines 29-32 state "`expirationDate` reaches the wire as EPOCH SECONDS: the model marks the member `@timestampFormat(\"date-time\")`, but the generated schema this emulator deserializes against carries no timestamp format", and every row sends an integer (`json!(expiration_date)` of epoch seconds, e.g. `PAST_2020`, `FUTURE_2100`). No test sends the RFC 3339 string, so nothing covers the form an SDK actually produces. For the secondary symptom: tests/all/deployment_parameters.rs lines 485-520 exercise the update path with a VALID tag map only (`ignored-at-update`), and tests/all/validation_messages.rs exercises malformed tags through `TagResource`, so the update path's missing TagsMap check is uncovered.

Relation to existing issues: the cross-service codegen issues (20261022T000000Z-smithy-macros-drops-timestampformat-from-member-schemas, 20261021T223000Z-scaffold-codegen-drops-timestampformat-from-member-schemas) describe the generator defect and state that "the only remedy available inside services/ is per-shape"; this issue names the affected member of THIS service, and is an INPUT-deserialization refusal (request rejected) rather than the output-rendering symptom those issues illustrate. The open marketplace-deployment issues for this operation cover different classes (fieldName in the boundary ValidationException body; the empty agreement store).

## Steps to Reproduce

1. Mount the emulator and seed an agreement the write can target (state::seed_agreements with catalog `AWSMarketplace`, agreement `agmt-held`, product `prod-held`), as the test harness does.
2. POST /catalogs/AWSMarketplace/products/prod-held/deployment-parameters with body {"agreementId":"agmt-held","deploymentParameter":{"name":"DbPassword","secretString":"s3cr3t"},"expirationDate":"2100-01-01T00:00:00Z"} — the RFC 3339 form the member's @timestampFormat("date-time") requires and the form an AWS SDK sends.
3. Observe HTTP 400 with a deserialization-failure body ("invalid timestamp") and no deployment parameter created, where AWS answers 200 with resourceArn/deploymentParameterId and stores the deadline.
4. Repeat the same request with {"expirationDate": 4102444800} (a JSON number) and observe HTTP 200 — the emulator accepts a wire form the modelled member does not admit.
5. Secondary symptom: create a parameter (name `DbPassword`) successfully, then POST the same catalog/product/agreementId/name again with {"tags":{"bad key!":"v"}} and observe HTTP 200 instead of the 400 ValidationException the modelled TagKey pattern requires (the same body sent for a NEW name is refused 400, so the two paths disagree).
