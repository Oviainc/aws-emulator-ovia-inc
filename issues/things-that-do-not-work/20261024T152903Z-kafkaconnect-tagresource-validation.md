Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# Tag keys and values are stored with no AWS tag rules: the reserved aws: prefix and illegal characters are accepted

_Filed: 2026-10-24T15:29:03Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (real service): a user-supplied tag key may not begin with the reserved `aws:` prefix — that namespace belongs to AWS system tags, and a `TagResource` (or a create carrying `tags`) that names such a key is refused with HTTP 400 `BadRequestException`, nothing written. Likewise only the standard tag character set is accepted (letters, numbers, spaces and `+ - = . _ : / @`); a key or value outside it is refused. `UntagResource` may not be used to strip an `aws:`-prefixed system tag either.

Actual (emulator): the only check on a tag write is how many entries the merged set would hold. `TagResource` with `{"aws:cloudformation:stack-name": "mine"}` is accepted (204, nothing refused), the key is persisted, and `ListTagsForResource` then reports an `aws:`-prefixed tag the real service would never let a caller create — a client that filters or asserts on system tags sees state AWS cannot produce. The same unchecked path serves the `tags` member of CreateConnector / CreateCustomPlugin / CreateWorkerConfiguration, and `UntagResource` removes any key it is given.

## Evidence

Spec: the model's `Tags` map constrains only sizes — `TagKey` len 1..128, `TagValue` len 0..256, map len 0..200, with no `@pattern` — so the framework cannot refuse these at the boundary and the handler must; `TagResource` declares `BadRequestException` (HTTP 400, "Bad request due to incorrect input"). Known semantics: the `aws:` tag prefix is reserved account-wide by AWS and user attempts to create/edit/delete such tags are rejected; the sibling MSK service documents exactly this for its `/v1/tags/{resourceArn}` pair, and other emulators in this repo enforce it (services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/tag_resource.rs line 160, services/service-com-amazonaws-b2bi-b2bi/src/state.rs line 687 `claims_reserved_prefix`).
Handler: src/ops/tag_resource.rs lines 174-200 passes `input.tags` straight to `crate::state::attach_tags`; src/state.rs lines 773-795 checks only `would_hold > MAX_TAGS_PER_RESOURCE` before inserting every key. src/ops/untag_resource.rs lines 171-184 / state.rs `remove_tags` (803-811) remove any named key. The creates store `input.tags.unwrap_or_default()` verbatim (create_connector.rs line 344, create_custom_plugin.rs line 281, create_worker_configuration.rs line 232).
Tests: tests/all/worker_configuration_delete_and_tags.rs covers per-key merge/removal, independence between resources, keys/values at the modelled length bounds and the 200-entry cap only — `grep -i 'aws:' tests/` finds no tag key carrying the reserved prefix, so no assertion contradicts refusing one.

## Steps to Reproduce

1. Create a worker configuration (or connector / custom plugin) and note its ARN.
2. POST /v1/tags/{arn} with body `{"tags": {"aws:cloudformation:stack-name": "mine"}}` — the emulator answers 2xx and stores the key; the real service answers 400 BadRequestException and writes nothing.
3. GET /v1/tags/{arn} — the emulator reports the `aws:`-prefixed tag back.
4. POST /v1/tags/{arn} with `{"tags": {"bad key!": "v"}}` (a character outside the allowed tag set) — accepted by the emulator, refused by the real service.
5. DELETE /v1/tags/{arn}?tagKeys=aws:cloudformation:stack-name — the emulator removes the reserved key instead of refusing.
