Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-iotevents
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-iotevents service-quotas-catalog-list-aws-default-service-quotas-kafka service-quotas-catalog-list-aws-default-service-quotas-kendra service-quotas-catalog-list-aws-default-service-quotas-kms service-quotas-catalog-list-aws-default-service-quotas-lakeformation service-quotas-catalog-list-aws-default-service-quotas-lex service-quotas-catalog-list-aws-default-service-quotas-license-manager service-quotas-catalog-list-aws-default-service-quotas-logs service-quotas-catalog-list-aws-default-service-quotas-medialive service-quotas-catalog-list-aws-default-service-quotas-mediapackage
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="DetectorModelsPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="BrokersPerCluster"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="ClustersPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="IndexesPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="AliasesPerKMSKey"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="CustomerManagedKMSKeys"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="GrantsPerKMSKey"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="DataLakeAdministratorsPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="BotsPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="LicenseConfigurationsPerAccount"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="DescribeLogStreamsThrottleLimitInTransactionsPerSecond"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="MetricFiltersPerLogGroup"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="SubscriptionFiltersPerLogGroup"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="ChannelsPerRegion"]
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# Service Quotas defaults for iotevents, kafka, kendra, kms, lakeformation, lex and 4 more ServiceCodes publish QuotaCodes AWS does not publish

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _WHAT THIS REPORT NOW COVERS. The defect closed here is the SURPLUS half of the original bundle: the fifteen `QuotaCode`s this catalog derived from a quota NAME for these ten services, which each service's recorded default-quota listing publishes nothing under. Those are the fourteen replay keys above (`medialive` and `mediapackage` share the string `ChannelsPerRegion`), and no response of this emulator carries any of them any more. The other half of the bundle — the 278 rows AWS publishes under its own `L-XXXXXXXX` codes, which this catalog still cannot source a code-to-quota pairing for — was SPLIT OUT, with its 278 replay keys, to `issues/things-that-do-not-work/20261030T005329Z-service-quotas-aws-coded-default-rows-for-ten-servicecodes-absent.md`, so none of it is recorded as working. That report also carries what this fix established about it. The three pairings the evidence below does state (`kafka/L-FAB9E493`, `kms/L-8A08DAEA`, `kms/L-275D92F3`) are now published under AWS's codes._

## Operation
ListAWSDefaultServiceQuotas

## Description
This issue bundles 10 causes — one per ServiceCode — of the same defect: the emulator's default-quota catalog is a small hand-written table rather than the service's published catalog. Each cause is its own ServiceCode and its own fixture, and a fixer must close and re-replay all 10 before submitting.

AWS (applies to every cause): ListAWSDefaultServiceQuotas(ServiceCode) answers 200 with `Quotas`, the complete published default-quota catalog for that service. Every entry is keyed by the AWS-assigned quota code of the form L-XXXXXXXX and carries ServiceCode, ServiceName, QuotaCode, QuotaArn (arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>), QuotaName, Description, Value, Unit, Adjustable and GlobalQuota, plus Period, UsageMetric and QuotaContext where AWS publishes them. Smithy: ListAWSDefaultServiceQuotasResponse.Quotas -> ServiceQuotaListDefinition. The per-ServiceCode catalog contents are recorded behavior only — each fixture's recorded 200 response is the reference.

Emulator (applies to every cause): 200 is returned, but the list holds only a few entries keyed by invented, human-readable codes AWS does not publish, and almost every real L- coded quota (with its Period/UsageMetric/QuotaContext members) is absent.

Why (applies to every cause): QuotaCode is the only stable key Service Quotas exposes and is the input to GetServiceQuota, GetAWSDefaultServiceQuota and RequestServiceQuotaIncrease. A client looking up a documented L- code finds nothing and concludes the quota does not exist; a client enumerating the list is handed codes no other API accepts and values that are not the service defaults, so throttling/limit pre-flight checks and capacity planning are wrong.

Fix (applies to every cause): the ListAWSDefaultServiceQuotas handler's quota-catalog state for Service Quotas — serve the published catalog keyed on AWS L- quota codes with all published members, and stop emitting codes AWS does not publish.

Cause 1 of 10: ServiceCode=iotevents — fixture service-quotas-catalog-list-aws-default-service-quotas-iotevents, step task#1. 26 L- coded quotas absent (e.g. L-DE2C2CBE, L-0B548925, L-279A67DA, L-18935129, L-134442DD); invented code 'DetectorModelsPerAccount'.
Cause 2 of 10: ServiceCode=kafka — fixture service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1. 7 L- coded quotas absent (L-B2FDE22A, L-36D29E8C, L-8F940D28, L-FAB9E493, L-9B45E609, L-03FD6822, L-EDD31C36); invented codes 'BrokersPerCluster', 'ClustersPerAccount'.
Cause 3 of 10: ServiceCode=kendra — fixture service-quotas-catalog-list-aws-default-service-quotas-kendra, step task#1. 28 L- coded quotas absent (e.g. L-7107C1BC, L-4E1777B8, L-C0EA1398, L-740FA277, L-31DA7514); invented code 'IndexesPerAccount'.
Cause 4 of 10: ServiceCode=kms — fixture service-quotas-catalog-list-aws-default-service-quotas-kms, step task#1. 57 L- coded quotas absent, including the per-API request-rate quotas with their Period and UsageMetric blocks (e.g. L-275D92F3 'UpdateCustomKeyStore request rate', L-8A08DAEA 'ListKeys request rate', L-171660E4); invented codes 'AliasesPerKMSKey', 'CustomerManagedKMSKeys', 'GrantsPerKMSKey'.
Cause 5 of 10: ServiceCode=lakeformation — fixture service-quotas-catalog-list-aws-default-service-quotas-lakeformation, step task#1. 7 L- coded quotas absent (L-3D55E97C, L-B0A837CA, L-3E3798DF, L-F165AF61, L-47097528, L-1343AC0E, L-9F3EFFAB); invented code 'DataLakeAdministratorsPerAccount'.
Cause 6 of 10: ServiceCode=lex — fixture service-quotas-catalog-list-aws-default-service-quotas-lex, step task#1. 13 L- coded quotas absent (e.g. L-824BBA1D, L-9030FC28, L-36FA8BD2, L-4F321B43, L-ED50DA7C); invented code 'BotsPerAccount'.
Cause 7 of 10: ServiceCode=license-manager — fixture service-quotas-catalog-list-aws-default-service-quotas-license-manager, step task#1. 25 L- coded quotas absent (e.g. L-30F617EC, L-CDB75D7A, L-A6872F54, L-9FC671A7, L-46604BA1); invented code 'LicenseConfigurationsPerAccount'.
Cause 8 of 10: ServiceCode=logs — fixture service-quotas-catalog-list-aws-default-service-quotas-logs, step task#1. 69 L- coded quotas absent, including the per-API throttle quotas with their Period/UsageMetric (e.g. L-FB8CE9B0, L-9D43025B, L-F738B5C5, L-4A1D48B1); invented codes 'DescribeLogStreamsThrottleLimitInTransactionsPerSecond', 'MetricFiltersPerLogGroup', 'SubscriptionFiltersPerLogGroup'.
Cause 9 of 10: ServiceCode=medialive — fixture service-quotas-catalog-list-aws-default-service-quotas-medialive, step task#1. 24 L- coded quotas absent (e.g. L-9E4BC4C0, L-D1AFAF75, L-019F695A, L-BDF24E14, L-A825B11C); invented code 'ChannelsPerRegion'.
Cause 10 of 10: ServiceCode=mediapackage — fixture service-quotas-catalog-list-aws-default-service-quotas-mediapackage, step task#1. 22 L- coded quotas absent (e.g. L-81A8E99B, L-258E8DBA, L-66FFDBE4, L-1E11547D, L-F7CB14AC); invented code 'ChannelsPerRegion'.

Related: the open issue 20261029T192055Z-service-quotas-listawsdefaultservicequotas-serves-a-hand-written-default-quota-table.md names the same root cause but carries replay keys for other ServiceCodes only; none of the 10 causes above is covered by it.

## Evidence
Example (Cause 4, fixture service-quotas-catalog-list-aws-default-service-quotas-kms step task#1) — expected entry in the recorded 200 response, absent from the emulator's: {QuotaCode: L-8A08DAEA, ServiceCode: kms, ServiceName: 'AWS Key Management Service (AWS KMS)', QuotaName: 'ListKeys request rate', Description: 'Maximum ListKeys requests per second. When you reach this quota, KMS rejects requests for this operation for the remainder of the interval.', Value: 500, Unit: None, Adjustable: true, GlobalQuota: false, Period: {PeriodUnit: SECOND, PeriodValue: 1}, UsageMetric: {MetricNamespace: AWS/Usage, MetricName: CallCount, MetricStatisticRecommendation: Sum, MetricDimensions: {Service: KMS, Resource: ListKeys, Type: API, Class: None}}} -> replay: 'missing ... got None'. Entries keyed 'CustomerManagedKMSKeys', 'AliasesPerKMSKey', 'GrantsPerKMSKey' are returned instead -> 'extra'.
Example (Cause 2) $.Quotas[QuotaCode="L-FAB9E493"] expected ('Number of brokers per cluster', Value 30) -> got None; extra entries keyed 'BrokersPerCluster', 'ClustersPerAccount'.
Each cause is one replay cluster, verdict=mismatch, expected_status 200, actual_status 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. For each cause, call Service Quotas ListAWSDefaultServiceQuotas with that cause's ServiceCode (iotevents, kafka, kendra, kms, lakeformation, lex, license-manager, logs, medialive, mediapackage) — fixture service-quotas-catalog-list-aws-default-service-quotas-<ServiceCode>, step task#1.
3. Diff the returned Quotas list against the fixture's recorded 200 response: the L-XXXXXXXX keyed entries (and their Period/UsageMetric/QuotaContext members) are missing and invented-key entries are present.
4. Re-replay all 10 fixtures listed in Issue-Trace/trace_ids to confirm a fix.

## Resolution
The surplus half of every one of the ten causes is closed, and the rule is now stated where the catalog is built.

* RULE: a default quota is published only under the `QuotaCode` the service itself answers to — AWS's own code where the catalog can attribute that code to that quota, the descriptive code only where the service demonstrably answers to it, and otherwise the row is not published at all. `QuotaCode` is the sole key `GetAWSDefaultServiceQuota`, `GetServiceQuota` and `RequestServiceQuotaIncrease` take, and a code identifies ONE quota.
* ROOT CAUSE: `src/state.rs::seed_quota_catalog` let a row with no cited `.code("L-…")` fall through to `descriptive_quota_code`, which mints a stand-in from the quota's NAME. Legitimate only where the service answers to that string (`b2bi`, `notifications-contacts`, `acm/ACMCertificates`, `access-analyzer/AnalyzersPerRegion`); for these ten services the recording reports it as a surplus entry.
* CHANGE: those fifteen rows are no longer published; `kafka/L-FAB9E493` ("Number of brokers per cluster", 30), `kms/L-8A08DAEA` ("ListKeys request rate", 500) and `kms/L-275D92F3` ("UpdateCustomKeyStore request rate", 5) are published under AWS's codes; `src/state.rs::kms_request_rate` builds KMS's per-operation request-rate class once. The ten service codes still resolve through `SEED_SERVICE_CATALOG`, with an empty `Quotas` list where no row can be cited.
* TESTS: `services/service-com-amazonaws-servicequotas-servicequotasv20190624/tests/all/default_quota_published_codes.rs` pins the rule over the fifteen pairs and the ten services (no derived code in either listing; `400 NoSuchResourceException` from `GetAWSDefaultServiceQuota`, `GetServiceQuota` and `RequestServiceQuotaIncrease`; every published code of these services AWS-minted and well formed at two endpoints; the re-keyed rows and the whole KMS request-rate class reporting their published members through the listing and the single read).
* STILL OPEN: the 278 AWS-coded rows, re-filed with their replay keys as `issues/things-that-do-not-work/20261030T005329Z-service-quotas-aws-coded-default-rows-for-ten-servicecodes-absent.md`.
