Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the Service Quotas emulator

_Filed: 2026-10-21T22:28:32Z_
_Operation: service-com-amazonaws-servicequotas-servicequotasv20190624 | Class: stub_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 26 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero state, so no client workflow works at all. Implement a SMALL first slice: the read-the-quotas + request-an-increase loop that is the service's core workflow. All of these must compute their responses from persisted state — no canned, hardcoded or echoed responses, and no random IDs that are not stored.

Happy path, in order (awsJson1_1, `POST /` with `X-Amz-Target: ServiceQuotasV20190624.<Op>`):

1. **ListServices** — input `MaxResults` (1..100), `NextToken`; output `Services` (list of `ServiceInfo{ServiceCode, ServiceName}`), `NextToken`. Must return the seeded service catalog, paginated: `MaxResults` honored and `NextToken` resuming where the previous page stopped (omit `NextToken` on the last page).
2. **ListServiceQuotas** — input `ServiceCode` (required), optional `QuotaCode`, `MaxResults` (1..100), `NextToken`, `QuotaAppliedAtLevel`; output `Quotas` (list of `ServiceQuota`), `NextToken`. Must list the *applied* quotas for that service from state, i.e. default values overlaid with any value changed by an approved increase. Unknown `ServiceCode` -> `NoSuchResourceException` (404). Paginate like ListServices.
3. **GetServiceQuota** — input `ServiceCode`, `QuotaCode` (both required), optional `ContextId`; output `Quota` (`ServiceQuota{ServiceCode, ServiceName, QuotaArn, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota, QuotaAppliedAtLevel}`). Must return the single applied quota read out of state and agree field-for-field with the entry ListServiceQuotas returned for the same (ServiceCode, QuotaCode). Unknown pair -> `NoSuchResourceException`; malformed codes (length/pattern per the model) -> `IllegalArgumentException` (400).
4. **GetAWSDefaultServiceQuota** — same input; output `Quota`. Must return the *immutable default* value for the quota, i.e. it must NOT change after an increase request is applied, which is the whole point of having both this and GetServiceQuota.
5. **RequestServiceQuotaIncrease** — input `ServiceCode`, `QuotaCode`, `DesiredValue` (double 0..10000000000, all required), optional `ContextId`, `SupportCaseAllowed`; output `RequestedQuota` (`RequestedServiceQuotaChange{Id, ServiceCode, ServiceName, QuotaCode, QuotaName, DesiredValue, Status, Created, LastUpdated, Requester, QuotaArn, GlobalQuota, Unit, QuotaRequestedAtLevel}`). Must mint a stored request with a generated `Id`, `Status = PENDING`, and `Created`/`LastUpdated` timestamps, persisted so steps 6 and 7 can read it back. Unknown quota -> `NoSuchResourceException`; non-adjustable quota or `DesiredValue` below the current applied value / above the model range -> `IllegalArgumentException`.
6. **GetRequestedServiceQuotaChange** — input `RequestId` (required); output `RequestedQuota`. Must return exactly the request stored in step 5 (same `Id`, `DesiredValue`, `ServiceCode`, `QuotaCode`, `Status`). Unknown id -> `NoSuchResourceException` (404).
7. **ListRequestedServiceQuotaChangeHistoryByQuota** — input `ServiceCode`, `QuotaCode` (required), optional `Status`, `MaxResults`, `NextToken`; output `RequestedQuotas`, `NextToken`. Must list the stored requests for that quota, newest first, filtered by `Status` when supplied, and paginated.

## State Model

All state lives behind the service's existing state lock, keyed per account+region:

- `services: Map<ServiceCode(String), ServiceInfo{ service_code, service_name }>` — seeded catalog of a handful of real services (e.g. `vpc`/"Amazon Virtual Private Cloud (Amazon VPC)", `lambda`, `ec2`, `dynamodb`, `s3`), used by ListServices and to resolve `ServiceName` in every output.
- `default_quotas: Map<(ServiceCode, QuotaCode), Quota{ quota_code, quota_name, quota_arn, value: f64, unit, adjustable: bool, global_quota: bool, period: Option<QuotaPeriod>, usage_metric: Option<MetricInfo>, description }>` — seeded, NEVER mutated; backs GetAWSDefaultServiceQuota and ListAWSDefaultServiceQuotas.
- `applied_quotas: Map<(ServiceCode, QuotaCode), AppliedQuota{ value: f64, quota_applied_at_level: AppliedLevelEnum, quota_context: Option<QuotaContextInfo> }>` — overlay written only when a request is applied; GetServiceQuota/ListServiceQuotas read `applied_quotas` falling back to `default_quotas`.
- `quota_requests: Map<RequestId(String), QuotaRequest{ id, service_code, quota_code, desired_value: f64, status: RequestStatus, created: Timestamp, last_updated: Timestamp, requester, case_id: Option<String>, quota_requested_at_level, context_id: Option<String> }>` plus an insertion-ordered index so history listings are newest-first and pagination tokens are stable.
- `quota_arn` built deterministically as `arn:aws:servicequotas:<region>:<account-id>:<service-code>/<quota-code>` so the same quota always reports the same ARN.

Pagination tokens must be opaque (base64 matching the modeled `^[a-zA-Z0-9/+]*={0,2}$`) and decode to a position in the ordered index; a token that does not decode -> `InvalidPaginationTokenException` (400).

Done = a client script can walk steps 1-7 end to end with correct, stateful bodies (not merely 2xx): the quota it gets back in step 3 matches the one listed in step 2, the default in step 4 is unchanged by step 5, and the request minted in step 5 is readable in steps 6 and 7.

Everything else — the Organizations quota-request *template* operations (AssociateServiceQuotaTemplate, DisassociateServiceQuotaTemplate, GetAssociationForServiceQuotaTemplate, Put/Get/Delete/ListServiceQuotaIncreaseRequest(s)(In|From)Template), the Auto Management operations (Start/Stop/Update/GetAutoManagementConfiguration), the utilization report operations (Start/GetQuotaUtilizationReport), CreateSupportCase, ListRequestedServiceQuotaChangeHistory, and the tagging trio (TagResource/UntagResource/ListTagsForResource) — stays stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-servicequotas-servicequotasv20190624/OPERATIONS.md` lists all 26 operations with their input/output members and errors; the wire protocol section documents `awsJson1_1`, `POST /`, `X-Amz-Target: ServiceQuotasV20190624.<OperationName>`, sigv4 name `servicequotas`. `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 26 handler files (e.g. `src/ops/get_service_quota.rs`, `src/ops/list_service_quotas.rs`, `src/ops/request_service_quota_increase.rs`), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501 and holding no state. Output shapes cited from the Smithy model: `ServiceQuota` (ServiceCode/ServiceName/QuotaArn/QuotaCode/QuotaName/Value/Unit/Adjustable/GlobalQuota/UsageMetric/Period/QuotaAppliedAtLevel/QuotaContext/Description), `RequestedServiceQuotaChange` (Id/CaseId/ServiceCode/QuotaCode/DesiredValue/Status{PENDING|CASE_OPENED|APPROVED|DENIED|NOT_APPROVED|CASE_CLOSED|INVALID_REQUEST}/Created/LastUpdated/Requester/QuotaArn/Unit), `ServiceInfo` (ServiceCode/ServiceName). Pagination traits `@paginated(inputToken=NextToken, outputToken=NextToken, items=...)` and `InvalidPaginationTokenException` are modeled on ListServices, ListServiceQuotas, ListAWSDefaultServiceQuotas and the history listings. No tests exist for any of these operations.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `servicequotas` client at it.
2. `aws service-quotas list-services` — expect a non-empty `Services` list of `{ServiceCode, ServiceName}`; actual: HTTP 501 NotImplemented.
3. `aws service-quotas list-services --max-results 2` then re-call with the returned `--next-token` — expect 2 items per page and the second page to continue (not repeat) the catalog.
4. `aws service-quotas list-service-quotas --service-code vpc` — expect the applied quotas for VPC with `QuotaCode`, `QuotaName`, `Value`, `Adjustable`, `QuotaArn`.
5. `aws service-quotas get-service-quota --service-code vpc --quota-code L-F678F1CE` — expect the same quota, field-for-field, as the matching entry from the previous step.
6. `aws service-quotas get-aws-default-service-quota --service-code vpc --quota-code L-F678F1CE` — note the default `Value`.
7. `aws service-quotas request-service-quota-increase --service-code vpc --quota-code L-F678F1CE --desired-value 10` — expect `RequestedQuota` with a generated `Id`, `Status=PENDING`, `DesiredValue=10.0`, `Created`/`LastUpdated` set.
8. `aws service-quotas get-requested-service-quota-change --request-id <Id from previous step>` — expect the same stored request back (not 404, not a fresh/echoed object).
9. `aws service-quotas list-requested-service-quota-change-history-by-quota --service-code vpc --quota-code L-F678F1CE` — expect the request from step 7 present; repeat with `--status PENDING` and expect it still present, with `--status APPROVED` and expect it filtered out.
10. `aws service-quotas get-aws-default-service-quota --service-code vpc --quota-code L-F678F1CE` again — expect the default `Value` unchanged from step 6.
11. `aws service-quotas get-service-quota --service-code vpc --quota-code NoSuchQuota` — expect `NoSuchResourceException` (HTTP 404), not 501.
