Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-services
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-services
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListServices|2xx->2xx|$.Services[*]
Issue-Commit: 5bf04d9c01ed3990fbed03ee0a0bc59641f482e6

# ListServices serves an incomplete integrated-service catalog with wrong ServiceName values

> _Note: this issue was identified against the code base at commit `5bf04d9c01ed3990fbed03ee0a0bc59641f482e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
Cause 1 of 1: the ListServices integrated-service catalog is both mis-labelled and short, so the returned Services list does not line up with AWS's at any index past the first few.

AWS: ListServices returns the Amazon Web Services services integrated with Service Quotas as ServiceInfo{ServiceCode, ServiceName}, where ServiceName is the service's display name - the same name ListAWSDefaultServiceQuotas/ListServiceQuotas report in ServiceQuota.ServiceName (Smithy: ListServices output {NextToken, Services: ServiceInfoListDefinition}, paginated on NextToken/MaxResults 1..100; ServiceInfo.ServiceCode @length 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$, ServiceInfo.ServiceName). Recorded behavior: the list is ordered by ServiceCode and holds every integrated ServiceCode.

Emulator: HTTP 200 with a Services list that diverges in two ways at once.
(a) ServiceName is wrong for entries it does have. For some codes it echoes the ServiceCode verbatim instead of a display name - `airflow-serverless` -> ServiceName 'airflow-serverless' where AWS reports 'Amazon MWAA Serverless (airflow-serverless)'; `aws-mcp` -> 'aws-mcp' where AWS reports 'AWS MCP'; `axis` -> 'axis' where AWS reports 'sagemaker-geospatial'. For others it reports a neighbouring service's name - `autoscaling-plans` -> 'AWS Auto Scaling' where AWS reports 'AWS Auto Scaling Plans'; `b2bi` -> 'AWS B2B Data Interchange' where AWS reports 'B2B Data Interchange'.
(b) ServiceCodes are missing from the catalog. Because the diff is positional, each absent code shifts everything after it: at index 37 AWS has `bedrock-agentcore` and the emulator already has `bedrock-websearch`; at index 44 AWS has `cleanrooms-ml` and the emulator has `cloud9`; at index 50 AWS has `cloudwatch`/'Amazon CloudWatch Omni' and the emulator has `cloudtrail`; at index 51 AWS has `cloudwatchpredictions` and the emulator has `cloudwatch`; at index 67 AWS has `connect-campaigns` and the emulator has `connect`. The replay reports 127 diverging Services indices in this one response.

Why: ListServices is the discovery entry point the API documents for finding a ServiceCode - a client walks it and then calls ListAWSDefaultServiceQuotas/ListServiceQuotas/GetServiceQuota with what it found. A ServiceCode the catalog omits is undiscoverable, and a ServiceName that is really the ServiceCode (or another service's name) makes the listing unusable for display and for matching a service by name.

Fix: the service-quotas crate's integrated-service catalog data that the ListServices handler reads - the same static data that backs ListAWSDefaultServiceQuotas' ServiceName - not the handler's paging. The ServiceName for a given ServiceCode must be the one value both listings report.

Traces: service-quotas-catalog-list-services#task#1.

Note on existing issues: the resolved issue 20261024T054353Z-service-quotas-listservices-pagination.md covers ListServices page SIZE, not the catalog contents, and is closed. The replay reports owned_by empty and stub_path empty for this cluster, so no open issue and no unimplemented-operation issue covers it.

## Evidence
Replay cluster: `service=service-quotas|ListServices|status=200->200`, fixture_id `service-quotas-catalog-list-services`, step_key `task#1`, verdict mismatch, expected_status 200, actual_status 200, affected_count 1, stub_path "", owned_by [].

divergence_key lists 127 diverging $.Services[n] indices (10, 29, 31, 32, 33, 37, 38, 39, 44, 46, 48, 50, 51, 52, 67, 68, 73, 75, 76, 80, 81, 83, 84, 89, 90, 91, 94, 95, 98, 99, 101-103, 106, 108, 110-112, 117, 119, 120, 126, 128, 131-135, 140, 142-144, 162, 171-176, 181-187, 189-191, 195, 196, 200, 205, 206, 210, 214-227, 229-231, 233, 234, 236-245, 247, 251, 253, 254, 256, 257, 259, 260, 262, 263, 268, 270-274, 276-278, 280).

Detail excerpts, expected vs got:
- `$.Services[10]: missing (expected {'ServiceCode': 'airflow-serverless', 'ServiceName': 'Amazon MWAA Serverless (airflow-serverless)'}, got {'ServiceCode': 'airflow-serverless', 'ServiceName': 'airflow-serverless'})`
- `$.Services[29]: missing (expected {'ServiceCode': 'autoscaling-plans', 'ServiceName': 'AWS Auto Scaling Plans'}, got {'ServiceCode': 'autoscaling-plans', 'ServiceName': 'AWS Auto Scaling'})`
- `$.Services[31]: missing (expected {'ServiceCode': 'aws-mcp', 'ServiceName': 'AWS MCP'}, got {'ServiceCode': 'aws-mcp', 'ServiceName': 'aws-mcp'})`
- `$.Services[32]: missing (expected {'ServiceCode': 'axis', 'ServiceName': 'sagemaker-geospatial'}, got {'ServiceCode': 'axis', 'ServiceName': 'axis'})`
- `$.Services[33]: missing (expected {'ServiceCode': 'b2bi', 'ServiceName': 'B2B Data Interchange'}, got {'ServiceCode': 'b2bi', 'ServiceName': 'AWS B2B Data Interchange'})`
- `$.Services[37]: missing (expected {'ServiceCode': 'bedrock-agentcore', 'ServiceName': 'Amazon Bedrock AgentCore'}, got {'ServiceCode': 'bedrock-websearch', 'ServiceName': 'bedrock-websearch'})`
- `$.Services[38]: missing (expected {'ServiceCode': 'bedrock-websearch', 'ServiceName': 'Amazon Bedrock Web Search'}, got {'ServiceCode': 'braket', 'ServiceName': 'Amazon Braket'})`
- `$.Services[44]: missing (expected {'ServiceCode': 'cleanrooms-ml', 'ServiceName': 'AWS Clean Rooms ML'}, got {'ServiceCode': 'cloud9', 'ServiceName': 'AWS Cloud9'})`
- `$.Services[50]: missing (expected {'ServiceCode': 'cloudwatch', 'ServiceName': 'Amazon CloudWatch Omni'}, got {'ServiceCode': 'cloudtrail', 'ServiceName': 'AWS CloudTrail'})`
- `$.Services[51]: missing (expected {'ServiceCode': 'cloudwatchpredictions', 'ServiceName': 'Amazon CloudWatch Anomaly Detection'}, got {'ServiceCode': 'cloudwatch', 'ServiceName': 'Amazon CloudWatch'})`
- `$.Services[67]: missing (expected {'ServiceCode': 'connect-campaigns', 'ServiceName': 'Amazon Connect outbound campaigns'}, got {'ServiceCode': 'connect', 'ServiceName': 'Amazon Connect'})`

Smithy evidence (services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json): ListServices input {NextToken @length 0..2048, MaxResults 1..100}, output {NextToken, Services: ServiceInfoListDefinition}, errors AccessDeniedException(403)/IllegalArgumentException(400)/InvalidPaginationTokenException(400)/ServiceException(500)/TooManyRequestsException(429); ServiceInfo = {ServiceCode @length 1..63 @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$, ServiceName}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire fixture corpus.
3. Step `task#1` of fixture `service-quotas-catalog-list-services` issues ListServices. The response is HTTP 200, but 127 positions in $.Services differ from the recording.
4. Compare $.Services[10], [29], [31], [32] and [33]: the ServiceName for airflow-serverless, aws-mcp and axis is the ServiceCode echoed back, and the ServiceName for autoscaling-plans and b2bi is a different service's display name.
5. Compare $.Services[37] onward: the emulator's entry at each index is a LATER ServiceCode than the recording's, because codes such as bedrock-agentcore, cleanrooms-ml, cloudwatchpredictions and connect-campaigns are absent from its catalog.
