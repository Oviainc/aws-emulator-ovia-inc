Issue-Severity: MEDIUM
Issue-Gap-Class: missing
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Implement GetOrganizationStatistics: no handler file at all, so the operation is unroutable

_Filed: 2026-10-24T02:18:46Z_
_Operation: GetOrganizationStatistics | Class: missing_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetOrganizationStatistics` is a modeled operation of the GuardDuty API service (GET /organization/statistics, 200, output `GetOrganizationStatisticsResponse` carrying `organizationDetails`), but the crate has NO handler for it: there is no `src/ops/get_organization_statistics.rs`, no generated types module for it, and nothing in the crate mentions the operation (`grep -r OrganizationStatistics services/service-com-amazonaws-guardduty-guarddutyapiservice/src` finds nothing). Unlike the 51 scaffolded operations, it is not even in the service's operation routing table, so a request to `GET /organization/statistics` falls through to the framework's unroutable-operation path.

Expected: HTTP 200 with `organizationDetails` computed from persisted state — `organizationStatistics.totalAccountsCount`, `memberAccountsCount`, `activeAccountsCount`, `enabledAccountsCount` and `countByFeature` derived from the detector's member records (`Detector::members`, which `CreateMembers`/`DisassociateMembers`/`ListMembers` already maintain) and the detector's own feature list (`Detector::features`), plus `updatedAt`. Documented failure modes are `BadRequestException` (400) and `InternalServerErrorException` (500).

Actual: the request names no modeled operation for this service, so the router answers its generic routing failure (rest protocol default: 501) with a body that is not one of GuardDuty's modeled error shapes — an SDK cannot even resolve it to `BadRequestException`/`InternalServerErrorException`, and no state is consulted.

## Evidence

Spec: `smithy_query(query="operation", operation="GetOrganizationStatistics")` shows the operation bound to the service with `http {method: GET, uri: /organization/statistics, code: 200}`, input `Unit`, output member `OrganizationDetails` (→ `OrganizationStatistics` with TotalAccountsCount / MemberAccountsCount / ActiveAccountsCount / EnabledAccountsCount / CountByFeature), errors BadRequestException(400) and InternalServerErrorException(500).

Emulator: `services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/SKIPPED_STUBS.md` line 21 records `// SKIPPED: GetOrganizationStatistics — unsupported shape construct: operation 'GetOrganizationStatistics' has a Unit/absent input, which the generator does not emit yet`, and that file's own preamble states such an operation "has neither [a typed skeleton nor a place in the operations(...) allow-list], and is answered by the framework's unroutable-operation path instead". `glob src/ops/*.rs` confirms there is no `get_organization_statistics.rs`; `src/service.rs` adds no `override_operation` route for it (it returns `None`).

Tests: no test under `tests/all/` exercises GetOrganizationStatistics (no match for the name anywhere in the crate). The state needed to compute the answer already exists and is tested — member records in `tests/all/member_accounts.rs` / `member_creation.rs`, detector features in `tests/all/detector_lifecycle.rs`.

## Steps to Reproduce

1. Start the emulator with the GuardDuty service mounted.
2. CreateDetector (Enable=true), then CreateMembers with one or two member AccountIds so the organization has observable membership.
3. Call GuardDuty GetOrganizationStatistics (aws guardduty get-organization-statistics, i.e. GET /organization/statistics).
4. Observe the framework's unroutable-operation answer (501, not a GuardDuty-modeled error shape) instead of HTTP 200 with organizationDetails.organizationStatistics counts derived from the members and features just created.
