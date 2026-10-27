Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c9e2d684fc826ee70e56effd17777b7bc5b4fec7

# An over-limit tagKeys selection on UntagResource escapes the tag surface's taxonomy and answers the ValidationException the operation does not model

_Filed: 2026-10-27T18:08:15Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c9e2d684fc826ee70e56effd17777b7bc5b4fec7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tag APIs of this service are supposed to answer their own error taxonomy (`BadRequestException` / `NotFoundException`), precisely because `UntagResource` does not model `ValidationException` at all. The boundary reworder implements that taxonomy for exactly two member paths: the `resourceArn` label, and the member named `tags` (TagResource's `tags` / RegisterSubscriptionProvider's `Tags`) when it blows its `@length` maximum. `UntagResource` carries its tag selection in a THIRD member, `tagKeys` (`TagKeyList`, `@length 0..50`), which `tag_api_refusal` does not recognise — so a removal naming 51 keys falls through to the generic branch and is answered with code `ValidationException` plus the AWS constraint envelope (`N validation error(s) detected: Value … at 'tagKeys' failed to satisfy constraint: …`). That is the one code the module's own rule 1 says this surface must not answer for `UntagResource`, and a client doing typed-error handling on this operation (catch `BadRequestException` / `NotFoundException` / `ResourceNotFoundException`) sees a code that is in neither its model nor the service's documented tag taxonomy. Note this is NOT the already-fixed malformed-`resourceArn` leg (cause 6 of the fixed 7-divergence issue): that path is handled at validation.rs:376; the `tagKeys` size path still is not.

## Evidence

Rule the crate states for itself — src/service/validation.rs:14-20: "1. **The tagging surface answers its own taxonomy.** The tag APIs (`/tags/{resourceArn}`) address their resource with the `resourceArn` label, and a value outside that label's modelled grammar is refused with `BadRequestException` — not the generic `ValidationException`, which `UntagResource` does not even model. A tag set past its modelled size limit is refused with `BadRequestException` too, wherever that tag map appears (`RegisterSubscriptionProvider`'s `Tags` as much as `TagResource`'s `tags`)."

What the code actually recognises — src/service/validation.rs:83: `const TAG_SET_MEMBER: &str = "tags";` with its doc at :80-82 ("The member a tag SET travels in: the `Tags` map of `RegisterSubscriptionProvider` and the `tags` map of `TagResource` … Both spell the same lowerCamel path, so one name covers them") — `tagKeys` is not among them; and src/service/validation.rs:376-382:
```
    if path == RESOURCE_ARN_MEMBER {
        return Some(&INVALID_ARN);
    }
    if path == TAG_SET_MEMBER && matches!(first.kind, ViolationKind::MaxLength(_)) {
        return Some(&TAG_LIMIT);
    }
    None
```
so the fallback at src/service/validation.rs:397-399 fires: `None => Some(Refused { code: VALIDATION_EXCEPTION.to_string(), message: envelope(violations)? })`, i.e. code `ValidationException` (src/service/validation.rs:73).

Model: `smithy_query(query="operation",operation="UntagResource")` lists errors `InternalServerException`, `ResourceNotFoundException` only — no `ValidationException` — and its input is `tagKeys` (`TagKeyList`, `"length": {"min": 0, "max": 50}`, `@httpQuery(tagKeys)`, required).

Test coverage: nothing pins the over-limit case — tests/all/tag_removal.rs:37-40 (`MAX_TAG_KEYS = 50`, "the modelled `tagKeys` list is `len 0..50`") exercises at most 50 keys, and tests/all/tag_not_found_status.rs:165 walks `for keys in [1_usize, 2, 50]`. Only the `tags` map's 51-entry case is pinned (tests/all/tag_not_found_status.rs:234-244, `TAG_LIMIT` = 400 `BadRequestException`).

## Steps to Reproduce

1. Seed/register one subscription provider so the ARN addresses a real resource (tests/all/harness.rs::seed_provider, or RegisterSubscriptionProvider).
2. Send `DELETE /tags/{resourceArn}?tagKeys=key-0&tagKeys=key-1&…` with 51 distinct `tagKeys` query values for that provider ARN.
3. Observe HTTP 400 with wire code `ValidationException` and the body message `1 validation error detected: Value … at 'tagKeys' failed to satisfy constraint: …`.
4. Compare with the same overflow on the sibling member: `PUT /tags/{resourceArn}` with 51 `tags` pairs answers HTTP 400 `BadRequestException` / "The resultant tag set must not have more than 50 user tags." — the taxonomy src/service/validation.rs:14-20 says the tag APIs answer, and the only one `UntagResource` models a code for.
