Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# Refuse includeMemberAccounts=true from an account that is not an organization management account

_Filed: 2026-10-24T10:53:32Z_
_Operation: ExportIdleRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `includeMemberAccounts` is the organization-wide switch — 'If your account is the management account or the delegated administrator of an organization, this parameter indicates whether to include recommendations for resources in all member accounts of the organization'. A caller that is neither (which is what this emulator presents: one standalone account, no organization recorded) is refused with HTTP 400 `InvalidParameterValueException` and no export job is created. This is the same boundary AWS enforces on the identical member of `UpdateEnrollmentStatus`, where the recorded refusal message is 'Cannot include member accounts. This could be because you are not an AWS Organization user or you are not the master account in the organization.'

Actual: the handler only refuses `includeMemberAccounts=true` when `accountIds` is also supplied (the documented mutual exclusion) and otherwise accepts it, creating an export job that claims to cover every member account of an organization the emulator does not have. A client (or onboarding script) that wrongly asks for an organization-wide export gets a 200 and a job id, and the documented 400 branch is never exercised.

## Evidence

Spec grounding: the operation's modeled `includeMemberAccounts` documentation conditions the parameter on the caller being the organization's management account or delegated administrator; the operation models `InvalidParameterValueException` (HTTP 400). Recorded AWS behaviour for the same member on a sibling operation is quoted in open issue 20261023T062035Z-compute-optimizer-wrong-rejection-class-for-out-of.md, Cause 3 of 3 (UpdateEnrollmentStatus refuses `includeMemberAccounts=true` from a non-management account with InvalidParameterValueException and leaves state unchanged); `crate::state::requested_account_id` and `CROSS_ACCOUNT_MESSAGE` already encode that this emulator presents one standalone account with no organization.

Handler: `src/ops/export_idle_recommendations.rs` lines 218-234 — the only use of `input.include_member_accounts` is the `accountIds` mutual-exclusion check; the flag is then discarded and never stored on the job (`crate::state::ExportJob` has no such field).

Test coverage status: `tests/all/recommendation_export_jobs.rs` (refusal table, lines ~360-435) covers the missing/blank bucket, the accountIds+includeMemberAccounts conflict and a malformed account id, but no case asserts a refusal for `includeMemberAccounts=true` alone; the open UpdateEnrollmentStatus issue covers only that operation, not this one.

## Steps to Reproduce

1. Opt the account in: `UpdateEnrollmentStatus(status="Active", includeMemberAccounts=false)`.
2. Call `ExportIdleRecommendations` with `{"s3DestinationConfig": {"bucket": "co-exports"}, "includeMemberAccounts": true}` using the emulator's standalone-account credentials (no `accountIds`).
3. Observe HTTP 200 with a `jobId` and `s3Destination`; real AWS refuses the request with HTTP 400 `InvalidParameterValueException` because the caller is not an organization management / delegated-administrator account.
4. Call `DescribeRecommendationExportJobs()` and observe the job that should never have been created.
