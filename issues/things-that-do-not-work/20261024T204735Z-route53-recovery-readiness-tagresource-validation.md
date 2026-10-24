Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# Reject tag keys/values that exceed the AWS tagging limits

_Filed: 2026-10-24T20:47:35Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource accepts any tag key and value as long as the resulting tag count is <= 50. AWS tagging limits are uniform across services: a tag key must be 1-128 characters, a tag value at most 256 characters, and the `aws:` prefix is reserved for AWS-managed tags (a caller-supplied `aws:`-prefixed key is refused).

Expected: POST /tags/{ResourceArn} with a 200-character tag key, an empty tag key, or a key beginning with `aws:` -> ValidationException (HTTP 400), nothing written.

Actual: the handler validates only the merged tag COUNT (`validate_added_tags`, ceiling 50) and then inserts every submitted pair, so an oversized/empty/reserved key is persisted and reported back by ListTagsForResources.

## Evidence

Handler: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/src/ops/tag_resource.rs lines 155-171 — the only pre-insert check is `crate::state::validate_added_tags`, which (src/state.rs:843-856) compares the merged key count against MAX_TAGS_PER_RESOURCE and inspects neither key nor value content. The model declares ValidationException (HTTP 400) for TagResource (OPERATIONS.md 'TagResource' errors), and the 128/256-character and reserved-`aws:`-prefix rules are the well-established AWS resource-tagging contract. Test coverage: tests/all/account_gate_and_not_found.rs:683 (`tags_written_through_tag_resource_are_reported_and_removed`) only covers well-formed tags; a repo-wide grep for `128`/`TagKey`/`aws:`-prefix tag assertions finds no test asserting these limits.

## Steps to Reproduce

1. Set ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE=1 and POST /cells {"cellName":"c1"} to get a cell ARN
2. POST /tags/<url-encoded cell ARN> with {"tags":{"<200-character key>":"v"}} — observe HTTP 200 instead of a 400 ValidationException
3. POST /tags/<same ARN> with {"tags":{"aws:reserved":"v"}} and with {"tags":{"":"v"}} — both accepted
4. GET /tags/<same ARN> — the oversized, empty and `aws:`-prefixed keys are reported as stored tags
