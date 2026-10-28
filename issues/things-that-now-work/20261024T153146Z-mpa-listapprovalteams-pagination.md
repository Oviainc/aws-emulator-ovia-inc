Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A NextToken this service never issued is accepted when it happens to be numeric, answering 200 with an empty page

_Filed: 2026-10-24T15:31:46Z_
_Operation: ListApprovalTeams | Class: pagination_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a pagination token the service did not issue is not a resumable position — AWS answers a forged or stale token with ValidationException (400). The crate itself holds this rule where the token is resolvable (ListPolicies and ListPolicyVersions reject a token the catalog/policy does not hold).

Actual: ListApprovalTeams decodes NextToken as a bare `u64` creation-order position and uses it as `team.seq > after` with no check that the position was ever issued. So `NextToken=999999` (or `NextToken=0`, or a token issued by a *different* list operation, since ListIdentitySources uses the identical seq grammar) is accepted and answered with HTTP 200 and an empty ApprovalTeams list, hiding every team from a client that replayed a stale token instead of telling it the token is invalid. Only non-numeric tokens are refused. ListIdentitySources (same u64 seq token) and ListResourcePolicies (bare usize offset) share the defect: any numeric value, including one past the end, is accepted.

## Evidence

Model: ListApprovalTeams is @paginated(inputToken=NextToken, outputToken=NextToken, items=ApprovalTeams) with ValidationException (400) declared. Handler: src/ops/list_approval_teams.rs — `input.next_token ... token.parse::<u64>().map_err(... ValidationException ...)`, i.e. the ONLY rejection is a parse failure; contrast src/ops/list_policies.rs, which refuses a token the catalog does not hold (`Some(token) if state.policy(token).is_some()`), and src/ops/list_policy_versions.rs, which refuses a version id the policy does not hold. Same looseness in src/ops/list_identity_sources.rs and src/ops/list_resource_policies.rs. Tests: tests/all/approval_team_lifecycle.rs only probes non-numeric tokens ["not-a-token", "12x", "-1", "%20"] for the 400, so a numeric never-issued token is untested.

## Steps to Reproduce

1. Create at least one approval team (record the caller's account as the organization management account first).
2. POST /approval-teams/?List with no NextToken and observe the team listed.
3. POST /approval-teams/?List&NextToken=999999.
4. Observe HTTP 200 with an empty ApprovalTeams array and no NextToken; expected 400 ValidationException for a token the service never issued.
5. Repeat with POST /identity-sources/?List&NextToken=999999 and POST /resource-policies/{ResourceArn}/?List&NextToken=999999 for the same outcome.
