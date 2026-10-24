Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 3a66b075702e523a87f5e33a7353c602a74f8e6d

# No wire path can make the caller the organization's management account, so CreateApprovalTeam (and CreateIdentitySource) can only ever answer 403

_Filed: 2026-10-24T19:15:41Z_
_Operation: CreateApprovalTeam | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `3a66b075702e523a87f5e33a7353c602a74f8e6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a client talking to the emulator over HTTP can reach a state in which CreateApprovalTeam succeeds. In real AWS the management-account gate is satisfiable: the account that calls organizations:CreateOrganization becomes the organization's management account, and MPA then lets that principal create approval teams and identity sources (200 with a stored team, followed by GetApprovalTeam/ListApprovalTeams reporting it).

Actual: the gate is unsatisfiable in the emulator. `ComAmazonawsMpaAwsfluffycoreserviceState::organization_management_account_id` is initialised to `None` by `Default` and is never assigned anywhere in `src/` — only `tests/all/approval_team_lifecycle.rs` sets it, by reaching into the Rust struct. `is_management_account()` therefore returns `false` for every caller forever, so `ManagementAccountGate` in `src/service.rs` (and the duplicate check inside each handler) answers HTTP 403 AccessDeniedException to EVERY CreateApprovalTeam and CreateIdentitySource request, whatever the body.

Consequence: the primary resource of this service can never be created through any public API path, so every downstream read is permanently empty or 404 — GetApprovalTeam, ListApprovalTeams, UpdateApprovalTeam, StartActiveApprovalTeamDeletion, DeleteInactiveApprovalTeamVersion, StartApprovalTeamBaseline, GetSession, CancelSession, ListResourcePolicies, GetResourcePolicy, TagResource/UntagResource/ListTagsForResource all have no reachable resource to act on. The Organizations emulator in this same repo already records a management account (services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/create_organization.rs sets `master_account_id = engine::emulator_account_id()`), but MPA never consults it and the two stores are not wired together.

Note this is NOT a request to drop the 403 for a caller that is genuinely not a management account (that refusal, and its ordering ahead of body validation, is recorded AWS behavior and is correct). The gap is that no caller can ever BE the management account: either MPA must honour the organization the Organizations emulator records, or the emulator's single account must be treated as the management account by default.

## Evidence

Spec/semantics: the Smithy model declares CreateApprovalTeam -> AccessDeniedException(403) and a 200 output carrying Arn/Name/VersionId/CreationTime, i.e. a successful create is a modelled outcome; well-established Organizations semantics make the account that calls CreateOrganization the management account of the new organization. Handler/state: src/state.rs lines ~1296 (`pub organization_management_account_id: Option<String>`), ~1330 (`organization_management_account_id: None` in `Default`), ~1348 (`is_management_account` = `self.organization_management_account_id.as_deref() == Some(account_id)`); grep over the whole crate shows the field is assigned only in tests/all/approval_team_lifecycle.rs:151. Gate: src/service.rs `management_account_gates!(create_approval_team::CreateApprovalTeam, create_identity_source::CreateIdentitySource)` + `ManagementAccountGate::denies_the_caller()`; handler copies at src/ops/create_approval_team.rs:226 and src/ops/create_identity_source.rs:214. Cross-service state that already exists: services/service-com-amazonaws-organizations-awsorganizationsv20161128/src/ops/create_organization.rs:252-268. Test coverage: the whole mpa behavioral suite bypasses the wire for this by mutating `store.lock().organization_management_account_id` in Rust, so no test covers an HTTP-only client, and nothing asserts that the gate is ever satisfiable from outside the process.

## Steps to Reproduce

1. Start the emulator with no pre-seeded state and point an MPA client at it (restJson1, signing name `mpa`).
2. Call organizations:CreateOrganization with the emulator's default credentials — it succeeds and records the caller's account as the organization's management (master) account.
3. Call mpa:CreateApprovalTeam with a well-formed body (Name matching ^[a-zA-Z0-9._-]+$, Description, ApprovalStrategy={MofN:{MinApprovalsRequired:1}}, one Approver, one Policy ARN from ListPolicies).
4. Observe HTTP 403 AccessDeniedException ('Action can only be performed by an organization's management account.'). Expected: HTTP 200 with Arn/Name/VersionId/CreationTime for a caller that is the management account.
5. Call mpa:ListApprovalTeams and mpa:GetApprovalTeam — nothing exists and nothing can ever exist; repeat step 3 with mpa:CreateIdentitySource for the same unconditional 403.
