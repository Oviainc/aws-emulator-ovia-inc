Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: dbfb16c0807859a588c77add63fc25981d8f50b1

# The reserved-tag-key refusal answers the deprecated TagPolicyException, while the same handler avoids the equally-deprecated TooManyTagsException

_Filed: 2026-10-28T22:35:40Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `dbfb16c0807859a588c77add63fc25981d8f50b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` and `UntagResource` refuse a tag key in the reserved `aws:` namespace with the modelled `TagPolicyException`. The model's own documentation for that shape says it is deprecated and names its replacement: "`TagPolicyException` is deprecated. See `ValidationException` instead."

The same crate already applies exactly that reasoning to the OTHER deprecated error shape bound to `TagResource`: for the 200-tag bound the handler deliberately does NOT answer `TooManyTagsException` because "That error is deprecated `See ServiceQuotaExceededException instead`, so the refusal is the quota error" — and it widens an unlisted shape into the operation's error enum through `extra_errors` to do so. Thirty lines earlier, in the same file, the reserved-key path returns the equally-deprecated `TagPolicyException` instead of its documented replacement. One file applies the deprecation rule to one shape and ignores it for the other, so a client that matches on `__type` sees a shape AWS documents as superseded, and loses the `reason` / `fieldList` members `ValidationException` carries (which is the form `CreateEnvironment` already uses for the identical reserved-key rule, through the same shared helper `crate::state::reserved_tag_key_failure`).

Both handlers that call the shared helper are affected: `TagResource` (tags) and `UntagResource` (tagKeys). `CreateEnvironment`, which enforces the same reservation, already answers `ValidationException`.

## Evidence

Smithy (smithy_query query="shape" shape="TagPolicyException"): doc = "<note> <p> <code>TagPolicyException</code> is deprecated. See <a href=\"...API_ValidationException.html\"> <code>ValidationException</code> </a> instead.</p> </note> <p>The request doesn't comply with IAM tag policy. Correct your request and then retry it.</p>"

Handler, the path that returns it anyway — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/tag_resource.rs:198-208:
```
        if let Some(refusal) = crate::state::reserved_tag_key_refusal(
            "tags",
            input.tags.keys().map(String::as_str),
        ) {
            return Err(
                TagResourceErrorModeled::TagPolicyException(TagPolicyException {
                    message: refusal,
                })
                .into(),
            );
```
and the same in services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/untag_resource.rs:176-186.

The OTHER side of the inconsistency, in the SAME file — src/ops/tag_resource.rs:230-245:
```
        // A resource carries at most 200 tags — the bound the model puts on the tag
        // map, and the one `TooManyTagsException` reports ("has more than 200 tags").
        // That error is deprecated "See ServiceQuotaExceededException instead", so the
        // refusal is the quota error.
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(
```
(`TooManyTagsException` is a declared variant of the very same enum — src/ops/tag_resource.rs:116 — and is left unused, exactly as this issue asks for `TagPolicyException`.)

The mechanism for widening an unlisted service shape into the enum is already in use here — src/ops/tag_resource.rs:107-119:
```
#[smithy_macros::operation_error(
    "com.amazonaws.evs#TagResource",
    extra_errors = ["InternalServerException"]
)]
```

The crate's own reading of the same rule for `CreateEnvironment` — src/ops/create_environment.rs:399-411 — routes it through `reserved_tag_key_failure` into an aggregated `ValidationException`.

Test coverage: tests/all/reserved_tag_keys.rs pins the current shape, and states its reason as the modelled error set alone — "`TagResource` and `UntagResource` declare `TagPolicyException` (HTTP 400) — the tag-policy refusal" (tests/all/reserved_tag_keys.rs:19-23) — which is the reason the model's deprecation sentence contradicts. The previously FIXED issue 20261024T012238Z-evs-tagresource-validation left the shape open ("refused with HTTP 400 (ValidationException, or the deprecated TagPolicyException)"), so nothing in the repo mandates the deprecated one.

## Steps to Reproduce

1. CreateEnvironment and build its ARN (arn:aws:evs:<region>:<account>:environment/<environmentId>).
2. Call TagResource with tags={"aws:owner":"team"}.
3. Observe HTTP 400 with body __type `TagPolicyException` — the shape the model documents as deprecated in favour of `ValidationException` — and no `reason`/`fieldList` members.
4. Call UntagResource with tagKeys=["aws:owner"] and observe the same `TagPolicyException`.
5. Compare with CreateEnvironment carrying tags={"aws:owner":"team"}, which refuses the same reserved key with `ValidationException` + fieldList, and with TagResource's 200-tag bound, which deliberately answers `ServiceQuotaExceededException` rather than the deprecated `TooManyTagsException`.
