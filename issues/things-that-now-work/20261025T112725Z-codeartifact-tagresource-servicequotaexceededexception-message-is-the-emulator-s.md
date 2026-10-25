Issue-Severity: MEDIUM
Issue-Gap-Class: fidelity
Issue-Trace-Ids: codeartifact.tag-resource-quota-exceeded-rejected

# CodeArtifact TagResource: the ServiceQuotaExceededException message is the emulator's own sentence, not the service's

> _Note: this file is a RE-FILED record. The original report's bytes were lost from the
> branch history that carried this work, so this file restates the same defect — its
> trace, its rule and its root cause — rather than reproducing the original wording. It is
> filed here, under things-that-now-work, because the defect it documents is now fixed;
> the fix is described under "Resolution" below._

## Operation
TagResource

## Description

A tag-quota refusal must state the rule the request broke in the words the service states
it in: an HTTP status, an error code and an error message are all requirements, and a
message that differs from the service's wording is a defect even when the status and the
code match.

`TagResource` refused an over-quota request with the right status (HTTP 402) and the right
modelled error (`ServiceQuotaExceededException`), but with a sentence the emulator had
invented for itself, describing the particular request rather than stating the bound.

Expected message: `The resultant tag set must not have more than 50 user tags.`

Actual message: `Cannot tag resource <arn>: the request would leave it holding <n> tags,
more than the maximum of 50 tags per resource.`

## Evidence

`codeartifact.tag-resource-quota-exceeded-rejected` compares the refusal's message, so it
diverged on the wording alone.

## Root cause

`services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/tag_resource.rs`
built the message inline with `format!`, and so did each create site that carries a `tags`
member (`create_domain.rs`, `create_repository.rs`, `create_package_group.rs`), each with
its own wording for the one quota.

## Resolution

The sentence now has ONE owner,
`crate::state::tag_quota_exceeded_message()`, which states the bound
(`MAX_TAGS_PER_RESOURCE`, the service's published per-resource tag restriction) in the
service's own words. `TagResource` and all three create sites that write a tag set answer
with it, so the refusal reads the same wherever the quota is enforced.

Pinned by
`services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/tests/all/resource_tags.rs`,
`a_tag_quota_refusal_states_the_rule_identically_at_every_write_site`: the bound passed in
one batch, far past it, and accumulated across batches, on a domain, a repository and a
package group, through `TagResource` and through each create carrying `tags` — every row
asserting HTTP 402, `ServiceQuotaExceededException` and that exact sentence.
