Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-kafka
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-kafka
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-36D29E8C"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-8F940D28"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-B2FDE22A"].Description
Issue-Related: 20261030T005329Z-service-quotas-aws-coded-default-rows-for-ten-servicecodes-absent.md
Issue-Commit: 5a373739f02617fd07fc9801baff6a41c1e94349

# ListAWSDefaultServiceQuotas kafka default catalog re-words quota Descriptions and omits broker and data-channel quotas

> _Note: this issue was identified against the code base at commit `5a373739f02617fd07fc9801baff6a41c1e94349`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
The emulator's Service Quotas default-quota catalog is a hand-seeded table. For ServiceCode 'kafka' (Amazon Managed Streaming for Kafka (MSK)) it answers HTTP 200, but the returned Quotas list neither carries AWS's per-quota Description text nor the full AWS default-quota row set. This issue is ONE cause: the seeded kafka rows of the default-quota catalog (both their text and their completeness).

Cause 1 of 1 - ServiceCode 'kafka' default-quota catalog rows (fixture service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1).

AWS: ListAWSDefaultServiceQuotas(ServiceCode) returns the service's complete AWS-default quota set. Smithy (services/service-com-amazonaws-servicequotas-servicequotasv20190624): ListAWSDefaultServiceQuotas -> ListAWSDefaultServiceQuotasResponse { Quotas: list<ServiceQuota>, NextToken }; input ServiceCode required, @length 1..63, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; ServiceQuota { ServiceCode, ServiceName, QuotaArn, QuotaCode (@length 1..128, pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$), QuotaName, Value (double, @range 0..10000000000), Unit, Adjustable (default false), GlobalQuota (default false), UsageMetric, Period, Description (@length 0..350) }; errors AccessDeniedException 403, IllegalArgumentException 400, InvalidPaginationTokenException 400, NoSuchResourceException 404, ServiceException 500, TooManyRequestsException 429. The model does not enumerate quota codes, their values or their text, so the CATALOG CONTENT is recorded behavior only and the recording is the requirement. For MSK the recording requires, among the kafka rows, the three quotas the emulator omits entirely - QuotaCode L-9B45E609 (QuotaName 'Number of data channels per cluster', Description 'The maximum number of data channels that can be created per cluster.', Adjustable true, GlobalQuota false, Unit 'None', Value 50.0), L-03FD6822 (QuotaName 'Number of brokers per KRaft cluster', Description 'The maximum number of brokers that a KRaft cluster can contain.', Adjustable true, GlobalQuota false, Unit 'None', Value 60.0) and L-EDD31C36 (QuotaName 'Number of brokers per account', Description 'The maximum number of brokers that can be created per account.', Adjustable true, GlobalQuota false, Unit 'None', Value 90.0) - each with ServiceCode 'kafka', ServiceName 'Amazon Managed Streaming for Kafka (MSK)' and QuotaArn built from the template arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>. It also requires AWS's own Description sentence for three rows the emulator does serve: L-B2FDE22A 'The maximum number of custom configurations that can be created per account.', L-36D29E8C 'The maximum number of revisions that can be made to a custom configuration.', L-8F940D28 'The maximum number of replicators that can be created per account.' In each of those sentences the whole wording is AWS's fixed text for that quota; the only request/state-derived parts of the response are the ServiceCode echoed from the request and the region inside QuotaArn.

Emulator: answers HTTP 200 with a kafka Quotas list that (a) re-words three Descriptions - $.Quotas[QuotaCode="L-B2FDE22A"].Description is 'The maximum number of configurations that an account can contain.' instead of AWS's 'The maximum number of custom configurations that can be created per account.'; $.Quotas[QuotaCode="L-36D29E8C"].Description is 'The maximum number of revisions that a configuration can contain.' instead of 'The maximum number of revisions that can be made to a custom configuration.'; $.Quotas[QuotaCode="L-8F940D28"].Description is 'The maximum number of replicators that an account can contain.' instead of 'The maximum number of replicators that can be created per account.' - and (b) omits the rows L-9B45E609, L-03FD6822 and L-EDD31C36 outright (replay reports each as 'missing (expected {...}, got None)'). No HTTP status or error code diverges: the fault is entirely in the response body.

Why: a client enumerating MSK limits gets a successful but wrong catalog. The three omitted quota codes cannot be discovered at all, so a caller cannot pass them to GetAWSDefaultServiceQuota, GetServiceQuota or RequestServiceQuotaIncrease (real AWS would accept them; the emulator's catalog makes them look like they do not exist), and capacity logic that reads 'Number of brokers per account' or 'Number of brokers per KRaft cluster' sees no limit where AWS reports 90 and 60. For the three re-worded rows, Description is the human-readable string a console, CLI table or quota dashboard renders, so the emulator surfaces text AWS never emits and any assertion on AWS's wording fails.

Fix: the service-quotas default-quota catalog seed data that the ListAWSDefaultServiceQuotas handler reads - the kafka rows specifically: add the three missing quota rows with their recorded QuotaCode/QuotaName/Description/Unit/Value/Adjustable/GlobalQuota/QuotaArn, and correct the Description text of L-B2FDE22A, L-36D29E8C and L-8F940D28. Not the operation's wire framing, pagination or serializer, which replay reports as correct (status 200, Quotas list shape intact).

Related open issues, none of which cover this cluster (replay reports it with owned_by EMPTY): 20261029T203936Z-service-quotas-listawsdefaultservicequotas-default-catalog-omits-quotas-batch-cassandra-cloud9.md, ...-cognito-identity-cognito.md, ...-ram-rds-redshift.md, 20261030T072417Z-...-access-analyzer.md, 20261030T072417Z-...-amplifyuibuilder-aoss.md, 20261030T104829Z-...-config-kinesis-monitoring-ses-signin.md and 20261030T104829Z-...-apigateway-appconfig.md. Each enumerates other ServiceCodes; none carries a key for ServiceCode 'kafka', so closing any of them would leave this fixture step still diverging.

Traces: service-quotas-catalog-list-aws-default-service-quotas-kafka#task#1 (cluster affected_count 1, verdict mismatch, stub_path empty, owned_by empty).

## Evidence
Replay of fixture service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1 (ListAWSDefaultServiceQuotas, ServiceCode 'kafka'): expected_status 200, actual_status 200, verdict mismatch - every difference is in the response body. divergence_key: service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-03FD6822"],$.Quotas[QuotaCode="L-36D29E8C"].Description,$.Quotas[QuotaCode="L-8F940D28"].Description,$.Quotas[QuotaCode="L-9B45E609"],$.Quotas[QuotaCode="L-B2FDE22A"].Description,$.Quotas[QuotaCode="L-EDD31C36"].

Divergence detail, verbatim from the replay:
- $.Quotas[QuotaCode="L-B2FDE22A"].Description: value_mismatch (expected 'The maximum number of custom configurations that can be created per account.', got 'The maximum number of configurations that an account can contain.')
- $.Quotas[QuotaCode="L-36D29E8C"].Description: value_mismatch (expected 'The maximum number of revisions that can be made to a custom configuration.', got 'The maximum number of revisions that a configuration can contain.')
- $.Quotas[QuotaCode="L-8F940D28"].Description: value_mismatch (expected 'The maximum number of replicators that can be created per account.', got 'The maximum number of replicators that an account can contain.')
- $.Quotas[QuotaCode="L-9B45E609"]: missing (expected {'Adjustable': True, 'Description': 'The maximum number of data channels that can be created per cluster.', 'GlobalQuota': False, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::kafka/L-9B45E609', 'QuotaCode': 'L-9B45E609', 'QuotaName': 'Number of data channels per cluster', 'ServiceCode': 'kafka', 'ServiceName': 'Amazon Managed Streaming for Kafka (MSK)', 'Unit': 'None', 'Value': 50.0}, got None)
- $.Quotas[QuotaCode="L-03FD6822"]: missing (expected {'Adjustable': True, 'Description': 'The maximum number of brokers that a KRaft cluster can contain.', 'GlobalQuota': False, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::kafka/L-03FD6822', 'QuotaCode': 'L-03FD6822', 'QuotaName': 'Number of brokers per KRaft cluster', 'ServiceCode': 'kafka', 'ServiceName': 'Amazon Managed Streaming for Kafka (MSK)', 'Unit': 'None', 'Value': 60.0}, got None)
- $.Quotas[QuotaCode="L-EDD31C36"]: missing (expected {'Adjustable': True, 'Description': 'The maximum number of brokers that can be created per account.', 'GlobalQuota': False, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::kafka/L-EDD31C36', 'QuotaCode': 'L-EDD31C36', 'QuotaName': 'Number of brokers per account', 'ServiceCode': 'kafka', 'ServiceName': 'Amazon Managed Streaming for Kafka (MSK)', 'Unit': 'None', 'Value': 90.0}, got None)

The 'us-west-2' in the QuotaArn values above is the recorded environment's region and is not itself the requirement; the requirement is the ARN template arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode> together with each row's QuotaCode, QuotaName, Description, Unit, Value, Adjustable and GlobalQuota. The Description strings are AWS's fixed text for those quotas and carry no per-run values.

Model evidence (Smithy, services/service-com-amazonaws-servicequotas-servicequotasv20190624): ListAWSDefaultServiceQuotas output Quotas -> ServiceQuotaListDefinition of ServiceQuota; ServiceQuota.Description @length 0..350 pattern ^.{0,350}$; ServiceQuota.Value double @range 0..10000000000; Adjustable/GlobalQuota boolean default false. The model does not enumerate quota codes or text, so the catalog content is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout (the data listener answers and /__admin/health returns 200).
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-kafka, step task#1 - ListAWSDefaultServiceQuotas with ServiceCode 'kafka'.
3. The response is HTTP 200; compare its Quotas list against the recording. Three Descriptions differ from AWS's text (L-B2FDE22A, L-36D29E8C, L-8F940D28) and three quota rows are absent entirely (L-9B45E609, L-03FD6822, L-EDD31C36).
4. A fix must make this fixture step replay clean: every recorded kafka quota present with its recorded QuotaCode, QuotaName, Description, Unit, Value, Adjustable, GlobalQuota and QuotaArn (region from the environment), and no re-worded Description text.
