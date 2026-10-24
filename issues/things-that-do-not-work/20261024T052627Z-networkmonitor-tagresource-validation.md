Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# TagResource accepts tag keys with the reserved aws: prefix

_Filed: 2026-10-24T05:26:27Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS reserves the `aws:` tag-key prefix for system tags; a customer request that tries to create or modify a tag whose key starts with `aws:` is refused (for a networkmonitor control-plane operation this surfaces as ValidationException / HTTP 400) and no tag is stored. The same reservation makes such keys non-removable through UntagResource.

Actual (emulator): `TagResource` performs no key-name validation at all — it only bounds the merged map size (`MAX_TAGS_PER_RESOURCE`) and then inserts every key/value pair verbatim. `TagResource(resourceArn=<monitor arn>, tags={"aws:owner": "me"})` answers 200 and the reserved-prefix tag is then reported by ListTagsForResource, GetMonitor and ListMonitors as if the caller owned it, and UntagResource happily removes it. A client that relies on the service to protect the reserved namespace sees tags AWS would never have accepted, and tag-policy code paths that handle the refusal are never exercised.

## Evidence

Spec/well-known semantics: AWS-wide tagging rule — keys prefixed `aws:` are reserved for AWS and cannot be created, edited or deleted by a caller; the networkmonitor model declares ValidationException (HTTP 400) on TagResource, so the refusal has a modeled home. Handler: `src/ops/tag_resource.rs` handler lines ~189-220 — the only pre-write check is the merged tag-count ceiling, followed by `tags.insert(key, value)` for every pair; no inspection of key text. `src/ops/untag_resource.rs` likewise removes any key named. Tests: `tests/all/resource_tags.rs::tag_resource_adds_exactly_the_named_pairs` and `a_tag_write_past_the_tag_map_maximum_is_refused_and_persists_nothing` cover the happy path and the count ceiling only; no test asserts the reserved prefix. Existing open networkmonitor tagging issues cover the 204 success status and the not-found error code, not key validation.

## Steps to Reproduce

1. CreateMonitor(monitorName="m1", aggregationPeriod=60) and note the returned monitorArn.
2. TagResource(resourceArn=<monitorArn>, tags={"aws:createdBy": "someone"}).
3. Observe a success response; expected ValidationException / HTTP 400 with nothing stored.
4. ListTagsForResource(resourceArn=<monitorArn>): the reserved-prefix key is reported as a customer tag.
5. UntagResource(resourceArn=<monitorArn>, tagKeys=["aws:createdBy"]) also succeeds, where AWS refuses removal of a reserved key.
