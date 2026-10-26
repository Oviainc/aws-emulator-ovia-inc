Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 409cf01b27185a74633e00014402d3a564807f11

# StartAutoManagement stores an ExclusionList naming quotas that do not exist, and the configuration is then reported with a QuotaCode but no QuotaName

_Filed: 2026-10-24T14:22:32Z_
_Operation: StartAutoManagement | Class: validation_

> _Note: this issue was identified against the code base at commit `409cf01b27185a74633e00014402d3a564807f11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartAutoManagement` records whatever `ExclusionList` the caller sends without resolving any of it against the quota catalog, so an exclusion naming a service code or quota code that does not exist is accepted with HTTP 200 and becomes part of the account's stored configuration.

Expected (AWS): the exclusion list names the quotas to exclude from Automatic Management by `(ServiceCode, QuotaCode)`; a code that names no quota is a resource that does not exist and is refused (`NoSuchResourceException`/`IllegalArgumentException`, both modeled on the operation), so every entry AWS later reports in `GetAutoManagementConfiguration.ExclusionList` is a real quota and carries both `QuotaCode` and `QuotaName` (`QuotaInfo.QuotaName`: "The Service Quotas name for the Amazon Web Services service monitored with Automatic Management").

Actual (emulator): the handler calls `crate::state::stored_exclusion_list(input.exclusion_list)`, which is a straight `HashMap -> BTreeMap` copy with no lookup, and the configuration is stored as given. `GetAutoManagementConfiguration` then reports the invented entry through `auto_management_exclusion_view`, which resolves the name with `default_quota(service_code, quota_code).map(...)` and emits `QuotaInfo { QuotaCode: Some("L-00000000"), QuotaName: None }` — a half-populated record AWS never returns, for a quota that does not exist. The same unvalidated path is taken by `UpdateAutoManagement` (`config.exclusion_list = stored_exclusion_list(input.exclusion_list)`), so a fix belongs in the shared helper.

Note for the fixer: the emulator's default-quota catalog is known to be incomplete (open issue `20261023T182924Z-service-quotas-default-quota-catalog-is-incomplete-and`), so validating exclusions against the catalog must land together with/after the catalog work, or real AWS quota codes the catalog still lacks would be refused.

## Evidence

Model: `StartAutoManagement` input `ExclusionList` is `map<ExcludedService, ExcludedQuotaList>` and the operation models both `IllegalArgumentException` (400) and `NoSuchResourceException` (404) (OPERATIONS.md lines 153-157); `GetAutoManagementConfiguration` returns `ExclusionList` as `map<string, list<QuotaInfo>>` where `QuotaInfo` pairs `QuotaCode` with `QuotaName` (smithy_query shape `QuotaInfo`), i.e. the service is expected to know the name of every excluded quota.

Handler/state: `src/ops/start_auto_management.rs` lines 146-154 stores `crate::state::stored_exclusion_list(input.exclusion_list)` with no catalog lookup; `src/state.rs` `stored_exclusion_list` (~line 1131) is `exclusion_list.unwrap_or_default().into_iter().collect()`; `src/state.rs` `auto_management_exclusion_view` (~lines 555-575) documents the consequence outright ("A code the catalog does not know is still reported, with no `QuotaName`"). `src/ops/update_auto_management.rs` lines 153-155 takes the same path.

Test coverage: `tests/all/auto_management.rs` row at line 215 (`exclusions: &[("lambda", &["L-00000000"])]`) asserts that an exclusion naming a nonexistent quota code is accepted and reported back, so the current lenient behaviour is pinned by a test and that row must move with the fix. No test exercises an unknown ServiceCode key, and no existing open issue covers Automatic Management input validation.

## Steps to Reproduce

1. Boot the emulator and POST `X-Amz-Target: ServiceQuotasV20190624.StartAutoManagement` with `{"OptInLevel":"ACCOUNT","OptInType":"NotifyOnly","ExclusionList":{"lambda":["L-00000000"],"not-a-service":["L-12345678"]}}` -> HTTP 200, nothing refused.
2. POST `GetAutoManagementConfiguration` `{}` -> the body echoes both exclusions; the `lambda` entry is `{"QuotaCode":"L-00000000"}` with no `QuotaName`, and `not-a-service` appears as a monitored service key although `ListServices` does not list it.
3. Repeat with `UpdateAutoManagement` `{"ExclusionList":{"not-a-service":["L-12345678"]}}` on a started configuration -> HTTP 200 and the same unresolvable entry is stored.
