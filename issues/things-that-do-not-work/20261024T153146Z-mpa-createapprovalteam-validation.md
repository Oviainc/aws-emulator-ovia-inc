Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# CreateApprovalTeam accepts Policies references no policy exists for, and reserved aws: tag keys

_Filed: 2026-10-24T15:31:46Z_
_Operation: CreateApprovalTeam | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two symptoms of one class — input the service rejects is stored instead.

1. Policies references are never resolved. The handler copies every PolicyReference$PolicyArn straight onto the stored team (`policies: input.policies.iter().map(|p| p.policy_arn.clone())`). An MPA policy is AWS-owned: nothing creates one, the catalog is seeded with exactly one entry (state::SEEDED_POLICY_CATALOG = IdentityCenter/MPAApprovalTeamPolicy) and ListPolicies/GetPolicyVersion answer out of it. A request naming any other pattern-conforming qualified policy ARN (e.g. .../IdentityCenter/NoSuchPolicy/$DEFAULT) therefore succeeds and the team is stored with a dangling attachment. The divergence is observable without any knowledge of the real catalog: GetApprovalTeam then reports that ARN in Policies, while ListResourcePolicies on the same team ARN silently drops it (state::resource_policies filter_maps references the catalog cannot resolve) and GetResourcePolicy answers 404 for it — two reads of the same attachment list that cannot both be right. Expected: a reference to a policy/version the service does not hold is refused with ValidationException (400, declared), and no team is stored.

2. Tags carried on create bypass the reserved-namespace rule. Create is the other path that writes tags; it checks only the count (>50 -> ValidationException) and stores keys verbatim, so a team can be created already carrying `aws:`-prefixed keys that TagResource should never have been able to write.

## Evidence

Model: CreateApprovalTeamRequest$Policies is required (length 1..10) of PolicyReference, whose PolicyArn is a QualifiedPolicyArn (pattern ^arn:.{1,63}:mpa:::aws:policy/<cat>/<name>/(?:[0-9]+|\$DEFAULT)$) — the pattern fixes the form, not existence; ValidationException (400) is declared for CreateApprovalTeam. Handler: src/ops/create_approval_team.rs — the only input checks are the management-account gate, ClientToken replay, duplicate Name, MAX_APPROVAL_TEAMS, tag count, repeated approver identity and MofN <= approver count; no state.policy()/policy_version() lookup appears anywhere in the file. State: src/state.rs attached_resource_policy() returns None for a reference the catalog cannot resolve and resource_policies() filter_maps it away, which is what makes GetApprovalTeam$Policies and ListResourcePolicies disagree. Tests: tests/all/approval_team_lifecycle.rs fabricates policy ARNs (fn policy_arn(n) -> .../MPAApprovalTeamPolicy{n}/...) that the seeded catalog does not hold and asserts create succeeds, so the current suite bakes the permissive behavior in rather than asserting the refusal; no test asserts aws:-prefixed tag keys either.

## Steps to Reproduce

1. Record the caller's account as the organization management account (CreateApprovalTeam is gated on it).
2. POST /approval-teams with Name="probe", Description="probe", ApprovalStrategy={MofN:{MinApprovalsRequired:1}}, one Approver, and Policies=[{PolicyArn: "arn:aws:mpa:::aws:policy/IdentityCenter/NoSuchPolicy/$DEFAULT"}], Tags={"aws:owner":"someone"}.
3. Observe HTTP 200 and a minted team ARN; expected 400 ValidationException (the policy version does not exist, and the tag key is in the reserved namespace).
4. GET /approval-teams/{Arn}: Policies reports the nonexistent policy ARN.
5. POST /resource-policies/{Arn}/?List: ResourcePolicies is empty, and POST /GetResourcePolicy for that policy name answers 404 — contradicting the team read.
