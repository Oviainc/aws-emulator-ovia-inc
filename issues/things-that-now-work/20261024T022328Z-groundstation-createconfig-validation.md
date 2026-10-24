Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# CreateConfig stores any tag map verbatim, including reserved aws: keys and over-long/over-many tags AWS refuses

_Filed: 2026-10-24T02:23:28Z_
_Operation: CreateConfig | Class: validation_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create path of this service takes the request's `tags` map and persists it with no checking at all — `create_config.rs` line ~202 does `tags: input.tags.unwrap_or_default()` — and `GetConfig` then reports it back. The model's `TagsMap` is a bare `map<String,String>` with no length, count or pattern constraint, so the framework's boundary validator lets every map through and the handler is the only place a tag rule could live.

Expected: AWS refuses a create whose tag map breaks the tagging rules the whole platform enforces, before any resource is minted — a key in the reserved `aws:` namespace (which only AWS may set), an empty or over-128-character key, an over-256-character value, or more than 50 tags on one resource. The refusal is a 4xx naming the bad parameter (`CreateConfig` models `InvalidParameterException`, HTTP 431, with `message` + `parameterName`; this service answers a body its validator rejects with HTTP 400 `BadRequestException`), and no config, id or ARN is created.

Actual: HTTP 200 with a `configId`/`configArn`, and the config is stored carrying the illegal tags. `GET /config/{configType}/{configId}` then serves them back, so a client that round-trips a resource description from a real account into the emulator — or one that depends on the refusal of an `aws:`-prefixed key — sees a success where AWS fails, and the emulator hands out an ARN for a config AWS would never have created. The same unchecked `input.tags.unwrap_or_default()` line exists in `create_mission_profile.rs` (~251), `create_dataflow_endpoint_group.rs` (~283) and `reserve_contact.rs` (~237), so fixing it once in a shared state helper covers the whole create surface (those three operations already carry their own open `validation` issues, so this one is filed against CreateConfig).

## Evidence

Spec: `smithy_query(query="shape", shape="TagsMap")` → `{"type": "map", "key": "String", "value": "String"}` — no `length`/`pattern`/size trait, so nothing in the model refuses a bad tag and the handler is the only gate. `smithy_query(query="operation", operation="CreateConfig")` declares `InvalidParameterException` (HTTP 431, members `message` + `parameterName`, doc "One or more parameters are not valid") precisely for a parameter AWS will not accept; `src/service.rs` `override_refusal` shows the other refusal envelope this service uses (HTTP 400 `BadRequestException` / "Invalid request body"). Well-established AWS semantics: the `aws:` tag-key prefix is reserved for AWS and cannot be set or removed by a caller; tag keys are 1–128 characters, values up to 256, with a 50-tag ceiling per resource — enforced by AWS tagging across services, Ground Station included.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_config.rs` line ~202 (`tags: input.tags.unwrap_or_default()`) is the only thing done with `tags`; `grep -n 'tags' src/` shows no key/value/count check anywhere in the crate (`state.rs` only stores `HashMap<String,String>` and `response_tags()` clones it back out).
Tests: `tests/all/lifecycle_writes.rs::a_read_serializes_tags_as_a_map_even_when_the_resource_carries_none`, `tests/all/scheduling_loop.rs` (~line 331-412) and `tests/all/dataflow_endpoint_groups.rs` (~line 299-347) only ever send legal tag maps (`{"stage": "created"}`, `{"mission": "demo"}`) and assert the round-trip, so no test asserts that an illegal tag map is accepted and adding the rule breaks no existing assertion.

## Steps to Reproduce

1. POST /config with a valid `configData` (e.g. `trackingConfig` with `autotrack: PREFERRED`), `name: "tagged tracking"` and `tags: {"aws:cloudformation:stack-name": "demo"}` — the emulator answers 200 with a `configId`/`configArn` where AWS refuses a caller-set `aws:`-prefixed tag key.
2. GET /config/tracking/{configId} — the reserved-prefix tag comes back in the `tags` map, proving it was persisted.
3. POST /config again with `tags` holding a 300-character key (and a second attempt with a 400-character value): both are accepted with 200 where AWS refuses the key/value length.
4. POST /config again with 60 distinct tag entries: accepted with 200 where AWS refuses more than 50 tags on a resource.
5. Repeat any of the above bodies against POST /missionprofile, POST /dataflowEndpointGroup and POST /contact (`tags` member) to see the same unchecked store on every create path.
