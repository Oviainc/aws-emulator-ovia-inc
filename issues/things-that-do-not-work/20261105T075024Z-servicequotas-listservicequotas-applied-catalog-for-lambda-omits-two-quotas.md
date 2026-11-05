Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-error-paths service-quotas-tag-applied-quota
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-14320ECE"]
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-25EEC0A4"].Description
Issue-Key: servicequotas|ListServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-BA29C22B"]
Issue-Commit: ce1e7fa07219b3a065b602a90626dda6f8aa6c34

# ListServiceQuotas applied catalog for lambda omits two quotas and paraphrases a burst-rate Description

> _Note: this issue was identified against the code base at commit `ce1e7fa07219b3a065b602a90626dda6f8aa6c34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServiceQuotas

## Description
Cause 1 of 1 — ListServiceQuotas for ServiceCode lambda returns an applied-quota catalog that is missing two rows AWS returns and that rewords the Description of a third.

AWS: ListServiceQuotas(ServiceCode='lambda') answers HTTP 200 with Quotas: list<ServiceQuota>, one row per APPLIED quota, each row carrying QuotaCode, QuotaName, Description, Value, Unit, Adjustable, GlobalQuota, QuotaAppliedAtLevel and a QuotaArn that — unlike the default catalog — includes the account id (arn:aws:servicequotas:<region>:<accountId>:lambda/<quotaCode>). The recording includes L-14320ECE ('Network bandwidth per execution environment', Description "The maximum sustained network bandwidth, in megabits per second, for a Lambda function's execution environment that is not attached to a VPC.", Unit 'Megabits/Second', Value 625, Adjustable true, GlobalQuota false, QuotaAppliedAtLevel ACCOUNT) and L-BA29C22B ('Rate of GetDurableExecutionHistory API requests', Unit 'None', Value 15, Adjustable true, QuotaAppliedAtLevel ACCOUNT). For L-25EEC0A4 the Description is 'The maximum number of ResumeMicrovm API requests that can be sent in one burst.' Smithy: ListServiceQuotas(ServiceCode required, optional QuotaCode/QuotaAppliedAtLevel/MaxResults/NextToken) -> { Quotas: list<ServiceQuota>, NextToken }; ServiceQuota.Description is a free-text member (max 350) and the model does not enumerate quota codes or their text, so the catalog content here is recorded behavior only and the recording is the requirement.
Emulator: answers 200, but $.Quotas contains no entry for QuotaCode L-14320ECE or L-BA29C22B, and serves L-25EEC0A4 with an emulator-authored Description 'The maximum burst rate of ResumeMicrovm API requests.'
Why: a client enumerating its applied Lambda quotas never sees the per-execution-environment network-bandwidth limit or the GetDurableExecutionHistory rate limit, so GetServiceQuota/RequestServiceQuotaIncrease on those codes has nothing to discover them from, and the quota text a console or CLI prints differs from AWS's for L-25EEC0A4.
Fix: the service-quotas applied-quota catalog seed for ServiceCode lambda (the data table the ListServiceQuotas handler reads) — add the two missing rows and restore AWS's Description text; the wire framing and status are already correct.
Traces: service-quotas-error-paths#task#4 (same cluster also covers service-quotas-tag-applied-quota#task#1).

Note: the open issues against ListAWSDefaultServiceQuotas cover the DEFAULT catalog (ListAWSDefaultServiceQuotas) for other service codes; none of them covers the applied catalog served by ListServiceQuotas for lambda — the replay reported owned_by empty for this cluster.

## Evidence
Replay of fixture service-quotas-error-paths, step task#4 (ListServiceQuotas, ServiceCode lambda): expected_status 200, actual_status 200, verdict mismatch. Body differences:
- $.Quotas[QuotaCode="L-25EEC0A4"].Description: value_mismatch — expected 'The maximum number of ResumeMicrovm API requests that can be sent in one burst.', got 'The maximum burst rate of ResumeMicrovm API requests.' (the whole sentence is fixed text for this quota; nothing in it is request- or state-derived).
- $.Quotas[QuotaCode="L-14320ECE"]: missing — expected {Adjustable True, Description "The maximum sustained network bandwidth, in megabits per second, for a Lambda function's execution environment that is not attached to a VPC.", GlobalQuota False, QuotaAppliedAtLevel 'ACCOUNT', QuotaArn 'arn:aws:servicequotas:us-west-2:000000000000:lambda/L-14320ECE', QuotaCode 'L-14320ECE', QuotaName 'Network bandwidth per execution environment', ServiceCode 'lambda', ServiceName 'AWS Lambda', Unit 'Megabits/Second', Value 625.0}, got None.
- $.Quotas[QuotaCode="L-BA29C22B"]: missing — expected {Adjustable True, Description 'The maximum number of GetDurableExecutionHistory API requests per second.', GlobalQuota False, QuotaAppliedAtLevel 'ACCOUNT', QuotaArn 'arn:aws:servicequotas:us-west-2:000000000000:lambda/L-BA29C22B', QuotaCode 'L-BA29C22B', QuotaName 'Rate of GetDurableExecutionHistory API requests', ServiceCode 'lambda', ServiceName 'AWS Lambda', Unit 'None', Value 15.0}, got None.
The region and account segments of the recorded QuotaArns are environment-specific; the requirement is the applied-quota ARN template arn:aws:servicequotas:<region>:<accountId>:lambda/<quotaCode> together with the QuotaCode/QuotaName/Description/Unit/Value/Adjustable/QuotaAppliedAtLevel of each row.
Cluster affected_count 2: service-quotas-error-paths#task#4 and service-quotas-tag-applied-quota#task#1 — the same two rows are missing from the applied catalog read by the tag-applied-quota fixture.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Replay fixture service-quotas-error-paths, step task#4 — ListServiceQuotas with ServiceCode 'lambda'. The call returns 200.
3. Search the returned Quotas list for QuotaCode 'L-14320ECE' and 'L-BA29C22B': both are absent, while the recording returns them with QuotaAppliedAtLevel ACCOUNT and Values 625.0 / 15.0.
4. Compare $.Quotas[QuotaCode="L-25EEC0A4"].Description with the recording to see the emulator-authored wording.
5. Re-replay service-quotas-error-paths#task#4 and the cluster member service-quotas-tag-applied-quota#task#1 to confirm a fix.
