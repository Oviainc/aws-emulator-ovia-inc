Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 55c71d6cc75db3eca6552dffd5fa68c4cb6ec346

# AccessDeniedException message embeds the raw ResourceArn, which can exceed the modelled 1024-character Message bound

_Filed: 2026-10-26T03:41:38Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `55c71d6cc75db3eca6552dffd5fa68c4cb6ec346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::authorize` builds the modelled `AccessDeniedException` by interpolating the caller ARN, the action and the RESOURCE verbatim. For the tagging operations the resource is the caller-supplied `ResourceArn`, modelled as `Arn` with `@length 20..2048`, and `authorize` runs BEFORE any account or existence check (`tag_resource.rs:165-171` precedes `validate_taggable_resource_arn` at `tag_resource.rs:186`), so a 2048-character ARN reaches the message builder. `AccessDeniedException$Message` is `@length 0..1024`, so a refused caller that passed a long (but pattern-valid) ARN gets a message roughly twice the modelled maximum.

The crate states the opposite rule for itself: every caller-supplied value a refusal echoes is clipped so the refusal stays inside the message member's own modelled bound. This path echoes an input of up to 2048 characters unclipped. The same applies to `UntagResource` and `ListTagsForResource`, which call the same helper with the same caller-supplied ARN.

## Evidence

Model (smithy_query shape="AccessDeniedException"): `"Message": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}}`. OPERATIONS.md:64 for TagResource: "`ResourceArn` (string, len 20..2048, pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$, required)".

src/state/authorization.rs:124-129:
```
    Err(AccessDeniedException {
        message: format!(
            "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
            caller.arn
        ),
    })
```
`resource` is the unmodified input — src/ops/tag_resource.rs:165-171 passes `&input.resource_arn`, and the account check that could reject an unusable ARN only runs afterwards (src/ops/tag_resource.rs:186-187 `crate::state::validate_taggable_resource_arn(&input.resource_arn)`).

The crate's own rule, src/state.rs:1163-1168: "The most characters of a caller-supplied value a refusal message echoes … so an echoed value is clipped to keep a refusal built from an outsized one inside the message's own modelled bound" — `clip_for_message` (src/state.rs:1173) is not applied here.

Test coverage: tests/all/front_door_authorization.rs and tests/all/untagging_authorization.rs assert the 403 and the message wording for short ARNs only; no test pins an over-long ARN, so nothing justifies the divergence.

## Steps to Reproduce

1. Create an IAM user with an inline policy that denies (or simply does not allow) `bcm-data-exports:TagResource`, and sign with that user's access key
2. POST / with X-Amz-Target: AWSBillingAndCostManagementDataExports.TagResource and `ResourceArn` = "arn:aws:bcm-data-exports:us-east-1:<emulator-account>:export/" followed by enough `[-a-zA-Z0-9/:_]` characters to reach ~2000 characters (still inside the modelled 20..2048 and the modelled pattern), with one ordinary tag
3. Observe the 403 `AccessDeniedException`: its `Message` is >2000 characters, past the modelled `@length 0..1024` of `AccessDeniedException$Message`
4. Repeat against UntagResource and ListTagsForResource — both call the same `crate::state::authorize` with the same caller-supplied ARN
