Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# An approver's PrimaryIdentitySourceArn is stored unchecked, so a value that is not an ARN at all becomes the identity source an approver authenticates through

_Filed: 2026-10-28T20:08:50Z_
_Operation: UpdateApprovalTeam | Class: validation_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both write paths that take an `ApprovalTeamRequestApprover` — `UpdateApprovalTeam$Approvers` and `CreateApprovalTeam$Approvers` — accept ANY string the member's `@length(max: 1000)` allows as `PrimaryIdentitySourceArn`, store it on the stored approver record, and report it back from `GetApprovalTeam` / `GetSession`. `PrimaryIdentitySourceArn` is documented as "Amazon Resource Name (ARN) for the identity source" and the model gives it NO `@pattern`, so the framework's constraint boundary passes `"not-an-arn"` straight through to the handler.

This is the exact member class this crate has one stated rule for, applied by six sibling handlers and by the service refusal seam: a member that should carry an ARN and does not is refused with `crate::state::invalid_arn()` (400, `INVALID_ARN_MESSAGE`). `ListTagsForResource`, `TagResource`, `UntagResource`, `GetResourcePolicy`, `ListResourcePolicies` and `DeleteIdentitySource` each call `crate::state::is_well_formed_arn` (or the stricter `is_identity_source_arn`) for their own pattern-less ARN member before anything else; `CreateIdentitySource` applies the very same reasoning to its pattern-less `Region` member, refusing a value that "the store would otherwise keep — and later report". `UpdateApprovalTeam`'s approver list IS validated here — for repeated identities, via `crate::state::repeated_identity` — so the list is inspected; only the ARN rule is skipped.

Consequences inside this crate: the team read reports an identity source ARN that addresses nothing (`GetApprovalTeam` would answer 400 for that same value if it were asked for as a resource ARN), and `DeleteIdentitySource`'s in-use guard (`state.team_arn_using_identity_source`, which compares `approver.primary_identity_source_arn == arn`) can never match such a value, so the invariant that guard exists to protect is silently vacuous for those approvers. Every existing test supplies a well-formed `arn:aws:mpa:<region>:<account>:identity-source/...` value for this member (e.g. `tests/all/approval_team_lifecycle.rs:58-63`, `tests/all/refusal_message_bound.rs:90-100`), so no test pins acceptance of a non-ARN value.

## Evidence

Model (smithy_query shape=ApprovalTeamRequestApprover — the input member of BOTH write paths):
  "PrimaryIdentitySourceArn": {"target": "String", "required": true, "length": {"max": 1000}, "doc": "<p>Amazon Resource Name (ARN) for the identity source. The identity source manages the user authentication for approvers.</p>"} — no `pattern`.

The crate's own rule, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:188-191:
  "//! * [`invalid_arn`] / [`INVALID_ARN_MESSAGE`] — the one message a member that should\n   //!   carry an Amazon Resource Name and does not is refused with, raised by a handler for\n   //!   a member the model gives no ARN `@pattern` and by `crate::service`'s refusal seam\n   //!   ([`is_malformed_arn_violation`]) for the members it does."

A sibling applying it, src/ops/list_tags_for_resource.rs:161-170:
  "// `ResourceArn` is a plain `String` with `length(max: 1000)` and NO modelled\n // pattern, so the boundary validator passes a value that is not an ARN at all:\n // this service's own ARN rule refuses it here ...\n if !crate::state::is_well_formed_arn(&input.resource_arn) {" (same code at src/ops/tag_resource.rs:173, src/ops/untag_resource.rs:165, src/ops/get_resource_policy.rs:189, src/ops/list_resource_policies.rs:191, src/ops/delete_identity_source.rs:210).

The same reasoning applied to the OTHER pattern-less member of a create, src/ops/create_identity_source.rs:267-279:
  "// `IamIdentityCenter$Region` names the Amazon Web Services Region ... the member is a plain string with only a length bound, so the\n // constraint boundary cannot decide it and the store would otherwise keep — and\n // later report — a value that names no Region at all.\n if !crate::state::is_aws_region(&parameters.region) {"

The update handler, src/ops/update_approval_team.rs:266-277 — the ONLY check made on the approver list:
  "if let Some(requested) = input.approvers.as_ref() {\n     if let Some(repeated) = crate::state::repeated_identity(requested.iter().map(|a| {\n         (a.primary_identity_id.as_str(), a.primary_identity_source_arn.as_str(),)" — no `is_well_formed_arn` / `is_identity_source_arn` / `state.identity_source(..)` call exists anywhere in update_approval_team.rs or create_approval_team.rs (grep for `identity_source` in src/ops/update_approval_team.rs returns only the three places the raw string is COPIED: lines 270, 370, 492).

The value is then persisted and reported, src/state.rs:3200-3204 (`invite_requested_approvers` -> `Approver::invited(... source_arn.to_string() ...)`) and src/ops/get_approval_team.rs:312:
  "primary_identity_source_arn: Some(approver.primary_identity_source_arn.clone()),"

The guard the unchecked value defeats, src/state.rs:2984-2998:
  "/// An approver is recorded with the identity source they are authenticated by\n /// (`Approver::primary_identity_source_arn`), so a source with such a reference is IN\n /// USE: removing it would leave the team pointing at a source this store no longer\n /// holds." ... "any(|approver| approver.primary_identity_source_arn == arn)"

Test coverage: every suite supplies a well-formed identity-source ARN for this member (tests/all/approval_team_lifecycle.rs:56-63 "`PrimaryIdentitySourceArn` is a plain string" + `format!("arn:aws:mpa:{REGION}:{}:identity-source/ssoins-{n:04}", ...)`; tests/all/refusal_message_bound.rs:87-100), and tests/all/identity_source_detail.rs:412-485 / tests/all/resource_arn_reads.rs:170 pin the 400 `INVALID_ARN_MESSAGE` answer for the malformed-ARN class on the resource-addressing members — so adding the missing check leaves the existing suites green.

## Steps to Reproduce

1. Create an approval team (harness sets the organization management account, as the existing suites do) and keep its `Arn`.
2. Call `PATCH /approval-teams/{Arn}` with body `{"Approvers":[{"PrimaryIdentityId":"identity-0001","PrimaryIdentitySourceArn":"not-an-arn"}]}` — the request satisfies every modelled constraint (`length(max: 1000)`, no pattern), so it reaches `handle`.
3. Observe HTTP 200 with a bumped `VersionId`: src/ops/update_approval_team.rs:439-468 writes the approver set with the raw string.
4. Call `GET /approval-teams/{Arn}`: the approver is reported with `"PrimaryIdentitySourceArn": "not-an-arn"` (src/ops/get_approval_team.rs:312).
5. Compare the same value on any sibling ARN member: `GET /tags/not-an-arn` and `DELETE /identity-sources/not-an-arn` both answer 400 `ValidationException` with `state::INVALID_ARN_MESSAGE` (src/ops/list_tags_for_resource.rs:165, src/ops/delete_identity_source.rs:210).
6. Same body through `POST /approval-teams` (`CreateApprovalTeam$Approvers`) stores the identical unchecked value — the create path has no ARN check either (src/ops/create_approval_team.rs:336-369).
