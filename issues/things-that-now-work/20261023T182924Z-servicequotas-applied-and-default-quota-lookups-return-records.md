Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-browse-services-and-quotas
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-browse-services-and-quotas service-quotas-error-paths service-quotas-tag-applied-quota service-quotas-service-code-pattern-validation
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Description
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.NextToken
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"].Adjustable
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].Adjustable
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"].Adjustable
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B2AA0F47"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].Adjustable
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"].Adjustable
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"].Unit
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"].Value
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-F55076EC"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-F55076EC"].QuotaName
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-F55076EC"].Value
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.UsageMetric.MetricDimensions.Resource
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# servicequotas applied and default quota lookups return records bound to the wrong quota, and ListServiceQuotas drops the pagination token

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceQuotas

## Description
The per-quota read path (GetServiceQuota / GetAWSDefaultServiceQuota / ListServiceQuotas) answers 200 but the ServiceQuota records it returns do not describe the quota that was asked for: some QuotaCodes are bound to a completely different quota's name, description, unit, value and adjustability, and others carry stale descriptive fields. ListServiceQuotas additionally omits the NextToken the recording returns, so the response claims to be the last page. This bundle has 6 CAUSES; all six must be closed before submitting.

SHARED MODEL BASIS: Smithy GetServiceQuota(ServiceCode @required, QuotaCode @required) -> Quota : ServiceQuota; GetAWSDefaultServiceQuota(ServiceCode, QuotaCode both @required) -> Quota : ServiceQuota; ListServiceQuotas(ServiceCode @required, optional QuotaCode/QuotaAppliedAtLevel/MaxResults/NextToken) -> {Quotas : list<ServiceQuota>, NextToken} with @paginated(inputToken: NextToken, outputToken: NextToken, items: Quotas, pageSize: MaxResults). ServiceQuota members: QuotaCode, QuotaName, Description (0..350), Value (double), Unit, Adjustable (bool default false), GlobalQuota, UsageMetric : MetricInfo{MetricNamespace, MetricName, MetricDimensions map, MetricStatisticRecommendation}, Period. The model fixes the SHAPE but not the catalog CONTENT, so the content requirement below is recorded behavior only. SHARED FIX TARGET: the servicequotas quota lookup/state layer that maps (ServiceCode, QuotaCode) to a ServiceQuota record, and the catalog rows it reads — a QuotaCode must resolve to its own quota definition.

Cause 1 of 6 — GetServiceQuota returns a stale Description for the lambda control-plane rate quota.
AWS: recorded behavior only — GetServiceQuota for the lambda control-plane API rate quota returns Quota.Description 'The maximum number of API requests per second (excluding invocation, GetFunction, and GetPolicy requests).'
Emulator: 200 with Quota.Description 'The maximum rate of control plane API requests, excluding invocation, GetFunction and GetPolicy requests.'
Why: Description is a modeled response member a client renders verbatim (console-style quota browsers, generated docs, drift diffs); a record that does not match AWS makes every such comparison report a false difference, and here it is a symptom of the catalog row not being the AWS row.
Fix: the servicequotas quota catalog row for that QuotaCode, read by the GetServiceQuota handler.
Traces: service-quotas-browse-services-and-quotas#task#3.

Cause 2 of 6 — GetAWSDefaultServiceQuota returns the same stale Description.
AWS: recorded behavior only — the default (AWS-published) view of the same quota carries the same Description as in cause 1.
Emulator: 200 with the same substituted Description text, so both the applied and the default read path serve the wrong row.
Why: as cause 1; a client that compares the applied value with the AWS default sees two records that agree with each other and disagree with AWS.
Fix: the same catalog row, via the GetAWSDefaultServiceQuota handler.
Traces: service-quotas-browse-services-and-quotas#task#4.

Cause 3 of 6 — ListServiceQuotas returns records bound to the WRONG quota for several QuotaCodes.
AWS: recorded behavior only — the recorded page contains, among others, QuotaCode L-72E0D058 = {QuotaName 'Number of concurrent MicroVM image builds', Description 'Number of concurrent MicroVM image builds per account per region', Unit 'Count', Value 10.0, Adjustable true}, L-F55076EC = {QuotaName 'Rate of capacity provider write API requests', Description 'The maximum combined rate (requests per second) for all capacity provider write APIs.', Value 1.0}, L-B82A30EA = {QuotaName 'Rate of SendDurableExecutionCallbackSuccess API requests', Unit 'None', Value 6000.0, Adjustable true}, plus L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B.
Emulator: 200 in which those QuotaCodes carry a different quota's definition entirely — L-72E0D058 comes back as {QuotaName 'Rate of GetFunction API requests', Description 'The maximum rate of GetFunction API requests.', Unit 'None', Value 100.0, Adjustable false}, L-F55076EC as {'Rate of GetPolicy API requests', Value 15.0}, L-B82A30EA as {'Function timeout', Unit 'Seconds', Value 900.0, Adjustable false}; QuotaName, Description, Unit, Value and Adjustable diverge together on 8 QuotaCodes.
Why: this is the worst form of the defect: the response is well-formed and the QuotaCode the client asked about is present, so the client trusts a limit (100 requests/second, timeout 900 s) that belongs to another quota. Capacity planning, admission control and quota-increase automation all act on the wrong number, and Adjustable flips so a client believes a non-adjustable quota can be raised.
Fix: the (ServiceCode, QuotaCode) -> ServiceQuota resolution in the servicequotas state/catalog layer feeding ListServiceQuotas — codes are being matched positionally or to the wrong row rather than by QuotaCode.
Traces: service-quotas-error-paths#task#4.

Cause 4 of 6 — ListServiceQuotas omits NextToken on a page that AWS continues.
AWS: Smithy marks ListServiceQuotas @paginated with outputToken NextToken (NextToken length 0..2048, pattern ^[a-zA-Z0-9/+]*={0,2}$); the recording returns a NextToken on this page, i.e. the result set continues.
Emulator: 200 with $.NextToken absent (`missing (expected '<opaque-pagination-token>', got None)`).
Why: an absent outputToken is the terminal signal for every paginator in every AWS SDK, so a client stops after the first page and silently processes a truncated quota list — no error is raised anywhere.
Fix: the pagination logic of the ListServiceQuotas handler: emit NextToken whenever more results remain (the token value itself is environment-specific and only its presence/shape is a requirement).
Traces: service-quotas-error-paths#task#4 — the same step as cause 3, so one replay of that step exercises both, but they are two independent fixes (record binding vs pagination).

Cause 5 of 6 — the same wrong-quota binding on the tag/applied-quota read path.
AWS: recorded behavior only — ListServiceQuotas in the tag-applied-quota fixture records the same correct definitions for L-72E0D058, L-F55076EC, L-B82A30EA, L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B (e.g. L-B82A30EA Value 6000.0, Adjustable true).
Emulator: 200 with the same substituted rows ('Rate of GetFunction API requests', 'Function timeout' Unit 'Seconds' Value 900.0, ...), confirming the defect is in the shared lookup rather than in one fixture's setup.
Why: as cause 3 — and it reproduces after a tagging workflow, so any client reading quotas it has just tagged gets the wrong limits.
Fix: as cause 3 (shared lookup), verified through this second fixture.
Traces: service-quotas-tag-applied-quota#task#1.

Cause 6 of 6 — GetServiceQuota returns a wrong Description AND a wrong UsageMetric dimension for the VPC per-Region quota.
AWS: recorded behavior only — GetServiceQuota for the VPCs-per-Region quota returns Description 'The maximum number of VPCs per Region. This quota is directly tied to the maximum number of internet gateways per Region.' and UsageMetric.MetricDimensions.Resource = 'VPCsPerRegion' (MetricInfo.MetricDimensions is the name/value pair identifying the CloudWatch metric).
Emulator: 200 with Description 'The maximum number of VPCs per Region in this account.' and UsageMetric.MetricDimensions.Resource = 'VPC'.
Why: MetricDimensions is not prose — a client uses Resource verbatim as a CloudWatch GetMetricData/GetMetricStatistics dimension value to read current usage against the quota. With 'VPC' instead of 'VPCsPerRegion' the metric query matches nothing and usage monitoring returns empty rather than failing.
Fix: the catalog row for that QuotaCode, specifically its UsageMetric.MetricDimensions and Description, read by the GetServiceQuota handler.
Traces: service-quotas-service-code-pattern-validation#task#2.

NOT INCLUDED (deliberately): the replay also reports five clusters on this service where the HTTP status (400) and the error code match the recording and only the error MESSAGE text differs (GetServiceQuota, ListServiceQuotas, RequestServiceQuotaIncrease, GetRequestedServiceQuotaChange, ListRequestedServiceQuotaChangeHistoryByQuota in fixtures service-quotas-error-paths and service-quotas-quota-change-history-and-support). No rule for those exact strings is stated by the model or the recording, so they are not filed here.

## Evidence
Replay of trace_set simparity-wire-aws-v1 (emulator up, 249 fixtures replayed, 16/270 steps passed, score 5.9%, state_reset_failures 0, bootstrap_signal false). All five clusters: expected 200 -> actual 200, verdict mismatch, service token 'servicequotas', no stub_path, no owned_by.

Cause 1 — service-quotas-browse-services-and-quotas#task#3, GetServiceQuota: `$.Quota.Description: value_mismatch (expected 'The maximum number of API requests per second (excluding invocation, GetFunction, and GetPolicy requests).', got 'The maximum rate of control plane API requests, excluding invocation, GetFunction and GetPolicy requests.')`.

Cause 2 — service-quotas-browse-services-and-quotas#task#4, GetAWSDefaultServiceQuota: identical `$.Quota.Description` value_mismatch.

Causes 3+4 — service-quotas-error-paths#task#4, ListServiceQuotas:
`$.NextToken: missing (expected '<opaque-pagination-token>', got None)`
`$.Quotas[QuotaCode="L-72E0D058"].QuotaName: value_mismatch (expected 'Number of concurrent MicroVM image builds', got 'Rate of GetFunction API requests')`
`$.Quotas[QuotaCode="L-72E0D058"].Description: value_mismatch (expected 'Number of concurrent MicroVM image builds per account per region', got 'The maximum rate of GetFunction API requests.')`
`$.Quotas[QuotaCode="L-72E0D058"].Unit: value_mismatch (expected 'Count', got 'None')`
`$.Quotas[QuotaCode="L-72E0D058"].Value: value_mismatch (expected 10.0, got 100.0)`
`$.Quotas[QuotaCode="L-72E0D058"].Adjustable: value_mismatch (expected True, got False)`
`$.Quotas[QuotaCode="L-F55076EC"].QuotaName: value_mismatch (expected 'Rate of capacity provider write API requests', got 'Rate of GetPolicy API requests')`, `.Value: expected 1.0, got 15.0`
`$.Quotas[QuotaCode="L-B82A30EA"].QuotaName: value_mismatch (expected 'Rate of SendDurableExecutionCallbackSuccess API requests', got 'Function timeout')`, `.Unit: expected 'None', got 'Seconds'`, `.Adjustable: expected True, got False`
`$.Quotas[QuotaCode="L-B2AA0F47"].Description: value_mismatch` (the cause-1 text)
plus the same five-field divergence on L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B (see divergence_key).

Cause 5 — service-quotas-tag-applied-quota#task#1, ListServiceQuotas: same set of QuotaCodes and fields, e.g. `$.Quotas[QuotaCode="L-B82A30EA"].Value: value_mismatch (expected 6000.0, got 900.0)`, `$.Quotas[QuotaCode="L-72E0D058"].Value: value_mismatch (expected 10.0, got 100.0)`.

Cause 6 — service-quotas-service-code-pattern-validation#task#2, GetServiceQuota: `$.Quota.Description: value_mismatch (expected 'The maximum number of VPCs per Region. This quota is directly tied to the maximum number of internet gateways per Region.', got 'The maximum number of VPCs per Region in this account.')` and `$.Quota.UsageMetric.MetricDimensions.Resource: value_mismatch (expected 'VPCsPerRegion', got 'VPC')`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture service-quotas-browse-services-and-quotas step task#3 — POST ServiceQuotasV20190624.GetServiceQuota for the lambda control-plane API rate quota and compare Quota.Description with the recording.
3. Cause 2: same fixture, step task#4 — GetAWSDefaultServiceQuota for the same (ServiceCode, QuotaCode); Quota.Description is substituted the same way.
4. Causes 3 and 4: replay fixture service-quotas-error-paths step task#4 — POST ListServiceQuotas and inspect the page: QuotaCodes L-72E0D058, L-F55076EC, L-B82A30EA (and L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B) carry another quota's QuotaName/Description/Unit/Value/Adjustable, and the response has no NextToken although the recorded page returns one.
5. Cause 5: replay fixture service-quotas-tag-applied-quota step task#1 — the same ListServiceQuotas page shows the same wrong records, so the fix must be in the shared (ServiceCode, QuotaCode) lookup, not in one fixture.
6. Cause 6: replay fixture service-quotas-service-code-pattern-validation step task#2 — GetServiceQuota for the VPCs-per-Region quota returns a different Description and UsageMetric.MetricDimensions.Resource 'VPC' instead of 'VPCsPerRegion'.
7. After fixing, re-replay all five fixtures above (steps 2-6) and confirm every cluster clears.

## Resolution

Causes 1, 2, 4 and 6 are fixed, and causes 3 and 5 are fixed for every QuotaCode whose quota this catalog can name: `state::seed_quota_catalog`'s lambda rows now carry the definition of the quota their code names (L-B2AA0F47, L-72E0D058, L-F55076EC, L-B82A30EA), the vpc VPCs-per-Region row carries its published Description and the `VPCsPerRegion` usage-metric dimension, and every listing of the service serves a bounded page through `state::paginate` (`state::UNASKED_PAGE_SIZE`) with a `NextToken` while rows remain. `tests/all/applied_quota_records.rs` pins both rules over a table. Fixtures service-quotas-browse-services-and-quotas and service-quotas-service-code-pattern-validation replay clean.

Remaining work, declined in this fix because no available value sources it (tracked here rather than silently closed):

* AWS's codes L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318 and L-BA29C22B name five lambda quotas whose recorded definitions this report does not state and the replay withholds. Rather than keep them bound to the wrong rows, the catalog no longer claims them (those rows take their derived code), so the pairs answer the modelled `NoSuchResourceException`. Publishing them needs their recorded `QuotaName`/`Description`/`Unit`/`Value`/`Adjustable`.
* `$.Quotas[QuotaCode="L-B82A30EA"].Description` — every other member of that row matches; AWS's published text is not available here, so the row carries this catalog's own wording.
* The 400-response `$.Message` clusters this report lists under "NOT INCLUDED (deliberately)" are still the first divergence of fixtures service-quotas-error-paths and service-quotas-tag-applied-quota.
