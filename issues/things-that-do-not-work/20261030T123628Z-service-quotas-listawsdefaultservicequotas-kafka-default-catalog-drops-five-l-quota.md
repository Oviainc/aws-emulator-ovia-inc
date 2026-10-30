Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-kafka
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-kafka
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-FAB9E493"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-FAB9E493"].ServiceName
Issue-Related: 20261030T005329Z-service-quotas-aws-coded-default-rows-for-ten-servicecodes-absent.md
Issue-Commit: c39abf24357da03f00ac6f740b99bcb6f841882f

# ListAWSDefaultServiceQuotas kafka default catalog drops five L- quota rows and misstates ServiceName

> _Note: this issue was identified against the code base at commit `c39abf24357da03f00ac6f740b99bcb6f841882f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
Cause 1 of 1: the emulator's built-in default-quota catalog for ServiceCode "kafka" (Amazon MSK) is incomplete at the row level and wrong at the field level, so ListAWSDefaultServiceQuotas answers 200 with a Quotas list that is a strict, differently-labelled subset of the AWS default catalog. Fixture: service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1.

AWS: Smithy model com.amazonaws.servicequotas#ServiceQuotasV20190624, operation ListAWSDefaultServiceQuotas (input ServiceCode required, @length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; output Quotas: ServiceQuotaListDefinition; paginated by NextToken/MaxResults). AWS returns, for a given ServiceCode, the complete set of that service's default quotas, one ServiceQuota per quota code. Each ServiceQuota (shape com.amazonaws.servicequotas#ServiceQuota) carries ServiceCode, ServiceName, QuotaArn, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota and Description (QuotaDescription, @length 0..350); the quota codes themselves, their QuotaName/Description text, their default Value and the ServiceName label are service-catalog data the model does not enumerate, so for the specific codes and strings this is recorded behavior only. The recording shows AWS serving ServiceName "Amazon Managed Streaming for Kafka (MSK)" for every kafka row, and serving a non-empty Description on each row (e.g. the broker-count quota is described as the maximum number of brokers a cluster can contain). Recorded status is 200 with no error.

Emulator: answers 200, but the Quotas list omits whole ServiceQuota entries for the kafka quota codes L-B2FDE22A (configurations per account), L-36D29E8C (revisions per configuration), L-8F940D28 (replicators per account), L-9B45E609 (data channels per cluster), L-03FD6822 and L-EDD31C36 — each resolves to None in the response. For the one row it does serve under L-FAB9E493 it omits Description entirely (null instead of the quota's description text) and serves ServiceName "Amazon Managed Streaming for Apache Kafka (Amazon MSK)" where AWS serves "Amazon Managed Streaming for Kafka (MSK)".

Why: a client enumerating MSK defaults — a console-style quota browser, a drift/compliance check that diffs applied quotas against defaults, or an increase-request workflow that looks a QuotaCode up before calling RequestServiceQuotaIncrease — sees six quota codes simply not exist. It concludes those quotas are unsupported for kafka, or it fails the lookup and never issues the increase request, instead of getting the default Value it would get from AWS. The rows it does get are also unusable for display or matching: a UI that renders Description shows an empty field, and any client that keys, groups or asserts on the ServiceName label (or renders it next to the quota) sees a service name AWS never emits, so a name-based join against ListServices output or a recorded expectation breaks.

Fix: the static default-quota catalog data for ServiceCode "kafka" in the service-quotas crate (the default-quota table that ListAWSDefaultServiceQuotas reads), not the handler's pagination or validation — status, shape and paging are all correct. The kafka entry needs the missing quota-code rows added with their AWS QuotaName/Description/Value/Unit/Adjustable/GlobalQuota, a Description populated on every existing row, and the service label corrected to the single ServiceName string AWS uses for that ServiceCode, applied consistently across every kafka row.

Related open issue: 20261030T005329Z-service-quotas-aws-coded-default-rows-for-ten-servicecodes-absent.md covers the kafka rows as wholly ABSENT entries (its Issue-Keys are whole-entry keys such as $.Quotas[QuotaCode="L-FAB9E493"]). It does not own this replay key: the divergence now reported is field-level on a row that IS present ($.Quotas[QuotaCode="L-FAB9E493"].Description and .ServiceName) alongside the still-missing codes, and the replay reports this cluster as unowned.

Traces: service-quotas-catalog-list-aws-default-service-quotas-kafka#task#1 (ListAWSDefaultServiceQuotas, ServiceCode=kafka).

## Evidence
Replay cluster (verdict=mismatch, expected_status=200, actual_status=200, affected_count=1), divergence_key service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-03FD6822"],$.Quotas[QuotaCode="L-36D29E8C"],$.Quotas[QuotaCode="L-8F940D28"],$.Quotas[QuotaCode="L-9B45E609"],$.Quotas[QuotaCode="L-B2FDE22A"],$.Quotas[QuotaCode="L-EDD31C36"],$.Quotas[QuotaCode="L-FAB9E493"].Description,$.Quotas[QuotaCode="L-FAB9E493"].ServiceName

Missing whole entries (expected a ServiceQuota, got None):
- $.Quotas[QuotaCode="L-B2FDE22A"] — QuotaName "Number of configurations per account", Adjustable true, GlobalQuota false, Unit "None", Value 100.0
- $.Quotas[QuotaCode="L-36D29E8C"] — QuotaName "Number of revisions per configuration", Adjustable true, Unit "None", Value 50.0
- $.Quotas[QuotaCode="L-8F940D28"] — QuotaName "Number of replicators per account", Adjustable true, Unit "None", Value 15.0
- $.Quotas[QuotaCode="L-9B45E609"] — QuotaName "Number of data channels per cluster"
- $.Quotas[QuotaCode="L-03FD6822"]
- $.Quotas[QuotaCode="L-EDD31C36"]

Field divergences on the row that IS served:
- $.Quotas[QuotaCode="L-FAB9E493"].Description: missing — expected the quota's description text ("The maximum number of brokers that a cluster can contain."), got None
- $.Quotas[QuotaCode="L-FAB9E493"].ServiceName: value_mismatch — expected "Amazon Managed Streaming for Kafka (MSK)", got "Amazon Managed Streaming for Apache Kafka (Amazon MSK)"

Fixed vs variable text: the ServiceName and Description strings above are the service's own catalog text for ServiceCode kafka and are fixed requirements of the response, not per-run values; the QuotaArn values in the recording (arn:aws:servicequotas:us-west-2::kafka/<QuotaCode>) vary only by region and quota code and are templated from the request's ServiceCode plus the row's QuotaCode. No error code or message is involved — both sides answer 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Or directly: call service-quotas ListAWSDefaultServiceQuotas with {"ServiceCode": "kafka"} against the emulator's data listener.
3. Observe HTTP 200 and inspect the Quotas list: quota codes L-B2FDE22A, L-36D29E8C, L-8F940D28, L-9B45E609, L-03FD6822 and L-EDD31C36 are absent, and the entry for L-FAB9E493 has no Description and ServiceName "Amazon Managed Streaming for Apache Kafka (Amazon MSK)".
4. The recorded AWS response for the same request contains all of those quota codes, a Description on each row, and ServiceName "Amazon Managed Streaming for Kafka (MSK)".
5. Fixture service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1.
