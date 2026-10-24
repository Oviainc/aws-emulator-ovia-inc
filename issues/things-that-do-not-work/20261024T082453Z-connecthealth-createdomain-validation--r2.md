Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# Reserved `aws:`-prefixed tag keys are accepted and persisted by CreateDomain (and TagResource) instead of being refused

_Filed: 2026-10-24T08:24:53Z_
_Operation: CreateDomain | Class: validation_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `aws:` tag-key prefix is reserved by AWS for system tags: a caller may never create or overwrite it, and every AWS tagging surface (tag-on-create and `TagResource` alike) refuses such a request with a client error (`ValidationException`, HTTP 400, text to the effect of "Tag keys cannot start with 'aws:'"), writing nothing.

This emulator applies only the modelled LENGTH bounds to a tag entry. `crate::service::tag_bounds::violation` (the one check shared by the two operations that accept a `TagMap`) walks each key against `TagKey`'s `@length 1..128` and each value against `TagValue`'s `@length 0..256` and nothing else — there is no prefix check anywhere in the crate.

Consequence: `CreateDomain` with `tags: {"aws:cloudformation:stack-name": "anything"}` answers 201 and stores the entry in the ARN-keyed tag store, after which `GetDomain.tags` and `ListTagsForResource` report a reserved system tag the caller set, and `TagResource` on an existing domain/subscription ARN accepts the same key with 204. A test that asserts the real service refuses a reserved key passes wrongly against the emulator, and the emulator lets user-set state masquerade as AWS-managed tags.

Expected: a `tags` map containing any key whose prefix is `aws:` (case-insensitively, as AWS treats the reserved prefix) is refused with a 400 `ValidationException` naming the offending key, and NOTHING is written — no domain is minted by `CreateDomain`, no tag entry is merged by `TagResource`.

## Evidence

Spec/well-known behavior: the `aws:` prefix is reserved across AWS tagging APIs (the same gap has been filed and accepted for sibling emulators, e.g. `20261024T074208Z-dsql-tagresource-validation.md` "TagResource accepts reserved `aws:`-prefixed tag keys instead of rejecting them" and `20261024T075111Z-iottwinmaker-tagresource-validation.md`). The model defines `TagMap` (key `TagKey` `@length 1..128`, value `TagValue` `@length 0..256`) with no pattern, so the prefix rule is a service-side rule a handler must enforce — exactly the case the crate already handles for length.

Handler: `services/service-com-amazonaws-connecthealth-connecthealth/src/service/tag_bounds.rs:137-153` — `violation()` returns only length violations (`checked(&M_TAG_KEY, key)` / `checked(&M_TAG_VALUE, value)`); no key-prefix test exists. Call sites: `src/ops/create_domain.rs:260-262` (then the tags are stored verbatim at `src/ops/create_domain.rs:318-324`, `store.tags_by_arn.insert(arn, tags)`) and `src/ops/tag_resource.rs:212-214` (then merged and stored at lines 219-236).

Test coverage: none — a grep of `tests/` for `aws:`/`reserved` finds only ARN literals (`tests/all/resource_tagging.rs:306,325`, `tests/all/list_tags_for_resource.rs:295,314`, etc.). `tests/all/tag_map_bounds.rs` and `src/service/tag_bounds.rs`'s unit tests cover only lengths and multi-byte keys, so the reserved prefix is unasserted either way.

## Steps to Reproduce

1. POST /domain with body {"name": "clinic", "tags": {"aws:cloudformation:stack-name": "mine"}}.
2. Observe HTTP 201 and a newly minted domain, instead of HTTP 400 `ValidationException` refusing the reserved tag key and creating nothing.
3. GET /domain/{domainId} and GET /tags/{arn} (ListTagsForResource): both report `aws:cloudformation:stack-name` as one of the domain's tags.
4. Create a second domain with no tags, then POST /tags/{arn} with body {"tags": {"aws:owner": "me"}}: observe HTTP 204 and the key stored, instead of a 400 `ValidationException` and no write.
5. Repeat step 4 with mixed case ("AWS:owner") and observe it is likewise accepted — AWS treats the reserved prefix case-insensitively.
