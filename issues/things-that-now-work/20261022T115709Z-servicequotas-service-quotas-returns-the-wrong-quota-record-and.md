Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-automatic-management-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-automatic-management-lifecycle service-quotas-browse-services-and-quotas service-quotas-error-paths service-quotas-service-code-pattern-validation service-quotas-tag-applied-quota
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[*]
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Adjustable
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Description
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.QuotaName
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Unit
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Value
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.Adjustable
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.Description
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.QuotaArn
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.QuotaName
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.Unit
Issue-Key: servicequotas|GetAWSDefaultServiceQuota|2xx->2xx|$.Quota.Value
Issue-Key: servicequotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[*]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.NextToken
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-4273958C"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-72E0D058"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B2AA0F47"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B430C318"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B82A30EA"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-F55076EC"]
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.Period
Issue-Key: servicequotas|GetServiceQuota|2xx->2xx|$.Quota.UsageMetric
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Service Quotas returns the wrong quota record and drops Description, Period, UsageMetric and pagination

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceQuotas

## Description
The ServiceQuota records Service Quotas serves are wrong in three independent ways: a lookup by QuotaCode resolves to a DIFFERENT quota, the records that are returned omit optional-but-published members (Description, Period, UsageMetric), and the applied-quota list is a single-element unpaginated stub. Seven causes.

Cause 1 of 7 — ListServiceQuotas returns the wrong quota at the head of the list and drops Description.
AWS: ListServiceQuotas(ServiceCode) returns 200 with Quotas listing the account's applied quotas for that service, each a full ServiceQuota (Smithy: ServiceQuota members ServiceCode, ServiceName, QuotaArn, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota, UsageMetric, Period, QuotaAppliedAtLevel, Description @length 0-350). For ServiceCode 'lambda' the recorded first element is QuotaCode L-B2AA0F47 ('Rate of control plane API requests...', Adjustable false, Unit 'None', Value 15.0, QuotaAppliedAtLevel 'ACCOUNT') with Description populated.
Emulator: 200, but $.Quotas[0] is L-2ACBD22F ('Function and layer storage', Adjustable true, Unit 'Gigabytes', Value 75.0) and carries no Description at all.
Why: a client that reads the list in order, or that treats the list as the authoritative applied-quota set, gets a quota it did not ask about with a different Adjustable/Unit/Value, so it mis-reports headroom and mis-decides whether an increase can even be requested.
Fix: the applied-quota state/catalog backing ListServiceQuotas, plus population of Description on the ServiceQuota serializer.
Traces: service-quotas-automatic-management-lifecycle#task#3 (also service-quotas-browse-services-and-quotas#task#2 in the same cluster).

Cause 2 of 7 — GetServiceQuota ignores the requested QuotaCode and returns a different quota.
AWS: GetServiceQuota(ServiceCode, QuotaCode) returns the one applied quota identified by that pair (Smithy: both members @required); for lambda/L-B2AA0F47 the recorded record is QuotaName 'Rate of control plane API requests (excludes invocation, GetFunction, and GetPolicy requests)', Adjustable false, Unit 'None', Value 15.0, Description populated.
Emulator: 200 with QuotaName 'Function and layer storage', Adjustable true, Unit 'Gigabytes', Value 75.0 and Description absent — i.e. the record for a different QuotaCode.
Why: this is a silent wrong answer on the single most-used read in the API: the client asked for one quota and got another's value, with no error to signal it. Any limit check built on it compares against the wrong number.
Fix: the GetServiceQuota handler's QuotaCode lookup (it must key on the requested code, not return a per-service default), and Description population.
Traces: service-quotas-browse-services-and-quotas#task#3.

Cause 3 of 7 — GetAWSDefaultServiceQuota returns a different quota AND builds the default-quota ARN with an account id.
AWS: GetAWSDefaultServiceQuota(ServiceCode, QuotaCode) returns the AWS default for exactly that pair. Two things hold in the recording: the record is the one for the requested QuotaCode (Description/QuotaName/Adjustable/Unit/Value as published), and the QuotaArn of an AWS *default* quota has an EMPTY account-id segment: arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>.
Emulator: 200 with the wrong quota's QuotaName/Adjustable/Unit/Value, no Description, and QuotaArn arn:aws:servicequotas:<region>:<accountId>:<serviceCode>/<quotaCode>.
Why: clients diff an applied quota against its AWS default to detect drift; a wrong default makes every quota look modified, and the account-bearing ARN breaks ARN equality against the real default ARN and any policy condition keyed on it.
Fix: the GetAWSDefaultServiceQuota handler's QuotaCode lookup, and the QuotaArn construction used on the AWS-default path (omit the account id there; applied quotas keep it).
Traces: service-quotas-browse-services-and-quotas#task#4.

Cause 4 of 7 — ListAWSDefaultServiceQuotas has the same wrong-record and ARN defects in list form.
AWS: ListAWSDefaultServiceQuotas('lambda') returns 200 whose elements are the published defaults; the recorded head element is L-2ACBD22F with QuotaName 'Rate of control plane API requests (excludes invocation, GetFunction, and GetPolicy requests)', Adjustable false, Unit 'None', Value 15.0, Description populated, and QuotaArn with the empty account segment.
Emulator: 200 whose head element carries QuotaName 'Function and layer storage', Adjustable true, Unit 'Gigabytes', Value 75.0, no Description, and an account-bearing QuotaArn.
Why: as cause 3, for the enumeration path rather than the single-record path.
Fix: the default-quota catalog rows for lambda and the shared AWS-default ARN construction.
Traces: service-quotas-browse-services-and-quotas#task#5.

Cause 5 of 7 — ListServiceQuotas returns a single applied quota and never a NextToken.
AWS: ListServiceQuotas is @paginated (inputToken NextToken, outputToken NextToken, items Quotas, pageSize MaxResults, MaxResults @range 1-100) and returns 200 with every applied quota for the service plus a NextToken when more pages remain. The recording for 'lambda' returns at least L-B2AA0F47, L-72E0D058, L-F55076EC, L-B82A30EA, L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B, each with QuotaAppliedAtLevel 'ACCOUNT', together with a NextToken.
Emulator: 200 with none of those nine QuotaCodes present and $.NextToken absent, so the response is both incomplete and signals that it is complete.
Why: a paginating client stops after one page believing it has the whole applied-quota set; the quotas it needs are simply never visible, and the pagination contract is never exercised.
Fix: the applied-quota state backing ListServiceQuotas and that handler's NextToken/MaxResults pagination.
Traces: service-quotas-error-paths#task#4.

Cause 6 of 7 — GetServiceQuota omits Description, Period and UsageMetric on a quota that publishes all three.
AWS: a ServiceQuota carries Period (QuotaPeriod{PeriodValue, PeriodUnit}) when the quota is rate-shaped and UsageMetric (MetricInfo{MetricNamespace, MetricName, MetricDimensions, MetricStatisticRecommendation}) when CloudWatch publishes usage for it, plus Description. The recording for vpc/L-F678F1CE carries Description, Period {PeriodUnit 'MINUTE', PeriodValue 5} and UsageMetric {MetricNamespace 'AWS/Usage', MetricName 'ResourceCount', MetricDimensions {...}, MetricStatisticRecommendation 'Maximum'}.
Emulator: 200 with $.Quota.Description, $.Quota.Period and $.Quota.UsageMetric all absent.
Why: Period is what tells a client the quota is a rate rather than a count, and UsageMetric is the documented way to wire a quota to a CloudWatch usage alarm. Without them a quota-monitoring client cannot build its alarm or interpret the value's dimension.
Fix: the ServiceQuota serializer / quota catalog — Period, UsageMetric and Description must be emitted when the quota defines them.
Traces: service-quotas-service-code-pattern-validation#task#2.

Cause 7 of 7 — the same single-quota applied-quota stub shows up on the tagging path.
AWS: ListServiceQuotas('lambda') on the tag-an-applied-quota flow returns the full applied set (L-B2AA0F47, L-72E0D058, L-F55076EC, L-B82A30EA, L-14320ECE, L-25EEC0A4, L-4273958C, L-B430C318, L-BA29C22B, each with QuotaAppliedAtLevel 'ACCOUNT').
Emulator: 200 with all nine absent.
Why: the tagging flow selects an applied quota from this list before tagging it; with the quota missing from the list the flow has nothing to tag, so TagResource/ListTagsForResource can never be reached against a real applied quota.
Fix: as cause 5 — the applied-quota state backing ListServiceQuotas.
Traces: service-quotas-tag-applied-quota#task#1.

## Evidence
Cause 1: fixture service-quotas-automatic-management-lifecycle step task#3, ListServiceQuotas — HTTP 200 both sides; $.Quotas[0] expected QuotaCode 'L-B2AA0F47' (QuotaName 'Rate of control plane API requests (excludes invocation, GetFunction, and GetPolicy requests)', Adjustable False, Unit 'None', Value 15.0, Description present), got QuotaCode 'L-2ACBD22F' (QuotaName 'Function and layer storage', Adjustable True, Unit 'Gigabytes', Value 75.0, no Description).
Cause 2: fixture service-quotas-browse-services-and-quotas step task#3, GetServiceQuota — HTTP 200 both sides; $.Quota.QuotaName expected 'Rate of control plane API requests (excludes invocation, GetFunction, and GetPolicy requests)', got 'Function and layer storage'; $.Quota.Adjustable expected False, got True; $.Quota.Unit expected 'None', got 'Gigabytes'; $.Quota.Value expected 15.0, got 75.0; $.Quota.Description expected present, got None.
Cause 3: fixture service-quotas-browse-services-and-quotas step task#4, GetAWSDefaultServiceQuota — HTTP 200 both sides; same QuotaName/Adjustable/Unit/Value/Description divergences, plus $.Quota.QuotaArn expected 'arn:aws:servicequotas:us-west-2::lambda/L-2ACBD22F' (empty account segment), got 'arn:aws:servicequotas:us-west-2:000000000000:lambda/L-2ACBD22F'.
Cause 4: fixture service-quotas-browse-services-and-quotas step task#5, ListAWSDefaultServiceQuotas — HTTP 200 both sides; $.Quotas[0] expected the L-2ACBD22F default record (Adjustable False, Unit 'None', Value 15.0, Description present, QuotaArn with empty account segment), got Adjustable True, Unit 'Gigabytes', Value 75.0, no Description, account-bearing QuotaArn.
Cause 5: fixture service-quotas-error-paths step task#4, ListServiceQuotas — HTTP 200 both sides; $.NextToken expected present (opaque token), got None; $.Quotas[QuotaCode="L-B2AA0F47"], [L-72E0D058], [L-F55076EC], [L-B82A30EA], [L-14320ECE], [L-25EEC0A4], [L-4273958C], [L-B430C318], [L-BA29C22B] expected present, got None.
Cause 6: fixture service-quotas-service-code-pattern-validation step task#2, GetServiceQuota (vpc/L-F678F1CE) — HTTP 200 both sides; $.Quota.Description expected present, got None; $.Quota.Period expected {'PeriodUnit': 'MINUTE', 'PeriodValue': 5}, got None; $.Quota.UsageMetric expected {'MetricNamespace': 'AWS/Usage', 'MetricName': 'ResourceCount', 'MetricDimensions': {...}, 'MetricStatisticRecommendation': 'Maximum'}, got None.
Cause 7: fixture service-quotas-tag-applied-quota step task#1, ListServiceQuotas — HTTP 200 both sides; the same nine lambda QuotaCodes expected present, got None.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: POST ListServiceQuotas ServiceCode 'lambda' and read $.Quotas[0] -> L-2ACBD22F with no Description instead of L-B2AA0F47. Fixture service-quotas-automatic-management-lifecycle, step task#3.
3. Cause 2: POST GetServiceQuota ServiceCode 'lambda', QuotaCode 'L-B2AA0F47' -> a record whose QuotaName/Unit/Value/Adjustable belong to another quota and no Description. Fixture service-quotas-browse-services-and-quotas, step task#3.
4. Cause 3: POST GetAWSDefaultServiceQuota ServiceCode 'lambda', QuotaCode 'L-2ACBD22F' -> wrong record plus $.Quota.QuotaArn containing an account id. Fixture service-quotas-browse-services-and-quotas, step task#4.
5. Cause 4: POST ListAWSDefaultServiceQuotas ServiceCode 'lambda' and read $.Quotas[0] -> same wrong record and account-bearing ARN. Fixture service-quotas-browse-services-and-quotas, step task#5.
6. Cause 5: POST ListServiceQuotas ServiceCode 'lambda' -> one quota, no $.NextToken; none of the nine recorded applied QuotaCodes present. Fixture service-quotas-error-paths, step task#4.
7. Cause 6: POST GetServiceQuota ServiceCode 'vpc', QuotaCode 'L-F678F1CE' -> $.Quota.Description/.Period/.UsageMetric absent. Fixture service-quotas-service-code-pattern-validation, step task#2.
8. Cause 7: POST ListServiceQuotas ServiceCode 'lambda' on the tagging flow -> applied quotas absent, nothing to tag. Fixture service-quotas-tag-applied-quota, step task#1.
