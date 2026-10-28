Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 40f95909aeff149ec63a31404ec4cdfbd5490fd0

# BatchIsAuthorized caps `entities` at 100 entities in total, where its own documentation allows 100 principals AND 100 resources

_Filed: 2026-10-28T23:21:20Z_
_Operation: BatchIsAuthorized | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `40f95909aeff149ec63a31404ec4cdfbd5490fd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchIsAuthorized`'s handler builds the batch's one entity slice with the shared `crate::state::cedar::entities_to_cedar_json`, which refuses any `entities` payload carrying more than `MAX_ENTITIES_PER_REQUEST` (100) entities in TOTAL, counted without regard to what kind of entity each one is.

The operation's own model documentation states a different capacity for this member: 100 principals *and* up to 100 resources, i.e. a slice of up to 200 entities. A batch that declares 100 principal entities and 100 resource entities — exactly the shape the documentation describes, and the shape a 30-request batch over many principals/resources needs — is therefore answered with a 400 `ValidationException` naming `/entities` instead of being evaluated.

The constant's own doc comment in this crate justifies the 100 only for the SINGLE-request operation ("How many entities one `IsAuthorized` request may carry", citing the published *Entities per authorization request* quota). Nothing in the crate addresses the figure `BatchIsAuthorized`'s documentation states for its own `entities` member, and no test pins the bound for this operation: the entity-count rows of `evaluator_input_bounds_refuse_instead_of_evaluating` and the `too_many_entities()` row of `a_refused_request_names_the_offending_member_in_field_list` both call `IsAuthorized`, never `BatchIsAuthorized`.

## Evidence

Model (smithy_query(query="doc", shape="BatchIsAuthorized")), verbatim:
  "The `entities` of a `BatchIsAuthorized` API request can contain up to 100 principals and up to 100 resources. The `requests` of a `BatchIsAuthorized` API request can contain up to 30 requests."
(The second sentence of that same pair IS honoured — `MAX_BATCH_REQUESTS = 30`, cedar.rs:272, checked at batch_is_authorized.rs:174 — so the first sentence is being read as if it did not state a figure.)

Handler, services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/batch_is_authorized.rs:240-245:
  // `entities` is a property of the BATCH, not of one request: one entity
  // slice is built once and every request is evaluated against it, ...
  let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref())
      .map_err(invalid)?;

The bound it reaches, src/state/cedar.rs:81-83 — scoped in its own words to the single-request operation:
  /// How many entities one `IsAuthorized` request may carry. The published
  /// Verified Permissions quota is *Entities per authorization request* — 100.
  pub const MAX_ENTITIES_PER_REQUEST: usize = 100;

and the refusal, src/state/cedar.rs:1337-1349:
  if items.len() > MAX_ENTITIES_PER_REQUEST {
      return Err(too_many(items.len()));
  }
  ...
  let count = value.as_array().map_or(0, Vec::len);
  if count > MAX_ENTITIES_PER_REQUEST {
      return Err(too_many(count));
  }
with `too_many` (cedar.rs:1321-1329) wording it as "it carries {count} entities, more than the {MAX_ENTITIES_PER_REQUEST} an authorization request may carry" at `paths::ENTITIES`.

Test coverage proving the batch case is unpinned:
- tests/all/policy_store_authorization.rs:1780-1798 — the `entity_cases` rows (`MAX_ENTITIES_PER_REQUEST` → 200, `+1` → 400) are sent with `call(&router, "IsAuthorized", &request)`.
- tests/all/policy_validation.rs:270-281 — the "more entities than one request may carry" row is also `"IsAuthorized"`; the three `"BatchIsAuthorized"` rows (lines 320-351) cover the `requests` size and missing/invalid triple members only.

## Steps to Reproduce

1. CreatePolicyStore(validationSettings={"mode":"OFF"}) and note the policyStoreId.
2. Build an `entities.entityList` with 100 items whose `identifier.entityType` is a principal type (e.g. `PhotoApp::User`, ids u0..u99) plus 100 items whose `identifier.entityType` is a resource type (e.g. `PhotoApp::Photo`, ids p0..p99) — 200 entities, which the operation documents as allowed (100 principals and up to 100 resources).
3. Call BatchIsAuthorized with that `entities` and a single well-formed request (principal `PhotoApp::User::u0`, action, resource `PhotoApp::Photo::p0`).
4. Observe HTTP 400 `ValidationException` with `fieldList[0].path == "/entities"` and the message "it carries 200 entities, more than the 100 an authorization request may carry"; no decision is returned.
5. Repeat with 100 entities total and observe HTTP 200, showing the cap is the whole-list count rather than the documented per-kind capacity.
