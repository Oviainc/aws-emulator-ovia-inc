Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Resources are not scoped to the request's Region: lists span every signing region and names collide across regions

_Filed: 2026-10-24T15:29:03Z_
_Operation: ListConnectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

KafkaConnect is a regional service: its endpoint is `kafkaconnect.<region>.amazonaws.com`, every resource lives in exactly one Region, and a call signed for Region B must not see the connectors, custom plugins or worker configurations created through Region A.

Expected (real service):
1. `ListConnectors` (and `ListCustomPlugins`, `ListWorkerConfigurations`, `ListConnectorOperations`) answered through the eu-west-1 endpoint reports ONLY the resources created in eu-west-1. A connector created in us-east-1 is absent — an empty page, not a listing of another Region's state.
2. A name is unique per Region: `CreateConnector`/`CreateCustomPlugin`/`CreateWorkerConfiguration` with the name `demo` in us-east-1 and again in eu-west-1 both succeed. The second call is NOT a `ConflictException`.
3. An ARN minted in another Region cannot be read, deleted or tagged through this Region's endpoint — `DescribeConnector`/`DeleteConnector`/`TagResource`/`ListTagsForResource` with a us-east-1 connector ARN on the eu-west-1 endpoint is a refusal, not a 200.
4. Service quotas are counted per Region.

Actual (emulator): the store is one flat set of `HashMap`s keyed by ARN only, and the request's signing Region is read *solely* to mint an ARN string at create time. `ListConnectors` walks `state.connectors.values()` with no Region predicate, so every connector of every Region is returned on every endpoint; `connector_named()` / `custom_plugin_named()` / `worker_configuration_named()` scan the whole store, so the same name in a second Region is answered 409 `ConflictException`; and the by-ARN reads/writes resolve any Region's ARN from any Region's endpoint. The resource quota is likewise counted store-wide.

## Evidence

Spec/known semantics: the model's `aws.api#service` gives `endpointPrefix: kafkaconnect` (per-Region endpoints) and every ARN this service mints carries a Region field — AWS MSK Connect resources are Region-scoped and `ListConnectors` is a per-Region listing.
Handler: src/ops/list_connectors.rs lines 174-183 — the only filter is `connector.name.starts_with(&prefix)`; `ctx` is explicitly discarded (`let _ = ctx;`), so the signing Region is never consulted. Same in list_custom_plugins.rs (171-180) and list_connector_operations.rs (175-188).
State: src/state.rs lines 144-149 states the choice outright — "the emulator serves a single account and does not partition state by region, so a resource created under one signing region is the same resource a call signed for another region reads"; `connector_named` (690), `custom_plugin_named` (676), `worker_configuration_named` (815), `tags_of` (733) and `connector_quota_reached` (670) all scan the single store. src/ops/create_connector.rs lines 202/318-323 shows `request_region` feeding only the ARN text.
Convention in this repo: the sibling MSK emulator keys state by Region (services/service-com-amazonaws-kafka-kafka/src/state.rs — per-Region name indexes and per-Region cluster counts, lines ~1416-1645), as do dynamodb, eks and observabilityadmin.
Tests: nothing asserts cross-Region visibility. Every suite signs with one scope (`eu-west-1`); tests/all/worker_configuration_create_and_describe.rs line 509-514 only checks that an ARN *string* carrying another Region is a 404 (a key mismatch, not Region scoping), so no existing assertion contradicts scoping the store by Region.

## Steps to Reproduce

1. POST /v1/connectors signed with a credential scope of `us-east-1/kafkaconnect` creating connector `demo` (with an ACTIVE custom plugin created the same way).
2. GET /v1/connectors signed with a credential scope of `eu-west-1/kafkaconnect` — the emulator returns the us-east-1 connector in `connectors`; the real service returns an empty page for eu-west-1.
3. POST /v1/connectors signed for `eu-west-1` with `connectorName: "demo"` — the emulator answers 409 ConflictException `Invalid parameter connectorName: A resource with this name already exists.`; the real service creates a second, independent eu-west-1 connector.
4. GET /v1/connectors/{the us-east-1 connector ARN} signed for `eu-west-1` — the emulator answers 200 with the connector; the real service does not resolve another Region's ARN.
5. GET /v1/tags/{the us-east-1 ARN} signed for `eu-west-1` — same cross-Region leak through the tag seam.
