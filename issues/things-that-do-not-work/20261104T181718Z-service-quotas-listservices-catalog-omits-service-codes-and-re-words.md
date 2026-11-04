Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-services
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-services
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListServices|2xx->2xx|$.Services[*]
Issue-Commit: 41f2e9525c7487ad1fe04cb3242a50ad61bb0d09

# ListServices catalog omits service codes and re-words ServiceName, shifting the whole listing

> _Note: this issue was identified against the code base at commit `41f2e9525c7487ad1fe04cb3242a50ad61bb0d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
ONE root cause.

Cause 1 of 1: `ListServices` serves a service catalog that is missing many of AWS's `ServiceCode` entries, and re-words the `ServiceName` of some entries it does serve. Because the listing is a positional array, every omission shifts all later entries, so 113 array positions diverge from one missing-entry fault.

AWS: Smithy `ListServices` (services/service-com-amazonaws-servicequotas-servicequotasv20190624) returns `Services: ServiceInfoListDefinition` of `ServiceInfo{ServiceCode (@length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$), ServiceName}`, paginated by NextToken/MaxResults. The model does not enumerate the catalog, so its CONTENT is recorded behavior only: AWS returns one entry per Service Quotas-integrated service, ordered by `ServiceCode`, with that service's registered display name as `ServiceName`. The recorded response includes `deadline`/'AWS Deadline Cloud', `dms`/'AWS Database Migration Service (AWS DMS)', `drs`/'ElasticDisasterRecovery', `ec2-ipam`/'IPAM', `ec2fastlaunch`/'EC2 Fast Launch', `eks-mcp`/'Amazon EKS MCP', `elasticfilesystem`/'Amazon Elastic File System (EFS)', `elemental-inference`/'AWS Elemental Inference', `finspace`/'Amazon FinSpace', `fis`/'AWS Fault Injection Service (AWS FIS)', `geo`/'Amazon Location Service (Amazon Location)' and `health-*`.

Emulator: those `ServiceCode` entries are absent from the `Services` list, so the ordered array slides: index 75 holds `datasync` where AWS has `deadline`, index 80 holds `devicefarm` where AWS has `dms`, index 83 holds `dlm` where AWS has `drs`, index 89 holds `dsql` where AWS has `ec2-ipam`, index 90 holds `dynamodb` where AWS has `ec2fastlaunch`, index 94 holds `ec2fastlaunch` where AWS has `eks-mcp`, index 98 holds `eks-mcp` where AWS has `elasticfilesystem`, index 101 holds `elasticbeanstalk` where AWS has `elemental-inference`, index 110 holds `es` where AWS has `finspace`, index 112 holds `evidently` where AWS has `fis`, index 119 holds `fsx` where AWS has `geo`. In addition the entries that are present are not always AWS's: `ec2fastlaunch` is served with ServiceName 'Amazon EC2 Fast Launch' where AWS registers 'EC2 Fast Launch', and `eks-mcp` is served with ServiceName 'eks-mcp' - the code echoed as the name - where AWS registers 'Amazon EKS MCP'. 113 array indices diverge in total ($.Services[40] through $.Services[303]).

Why: `ListServices` is the documented way a client discovers which `ServiceCode` values it may pass to `ListAWSDefaultServiceQuotas`, `ListServiceQuotas` and `RequestServiceQuotaIncrease`. A client that enumerates the catalog never sees `dms`, `drs`, `elasticfilesystem`, `fis`, `geo` or `finspace` and concludes those services have no quotas, so a quota sweep silently skips them; one that paginates and indexes or caches by position gets a different service than it recorded. `ServiceName` is the string shown to a human in a console or report, so a code echoed as its own name (`eks-mcp`) renders as an unnamed service.

Fix: the service catalog data behind the `ListServices` handler in the service-quotas crate - it must enumerate every ServiceCode AWS registers, with AWS's registered `ServiceName` for each, and keep the AWS ordering, rather than listing only the subset of codes the emulator happens to hold quota rows for.

Traces: fixture service-quotas-catalog-list-services, step task#1.

No open issue owns this step; the existing service-quotas catalog issues (e.g. 20261030T104829Z-service-quotas-listawsdefaultservicequotas-default-catalog-misbinds-and-omits-aws-quota.md) are all about `ListAWSDefaultServiceQuotas` rows, not the `ListServices` service list.

## Evidence
Expected HTTP 200, actual HTTP 200; 113 diverging keys in `$.Services`.

$.Services[75]: expected {'ServiceCode': 'deadline', 'ServiceName': 'AWS Deadline Cloud'}, got {'ServiceCode': 'datasync', 'ServiceName': 'AWS DataSync'}
$.Services[80]: expected {'ServiceCode': 'dms', 'ServiceName': 'AWS Database Migration Service (AWS DMS)'}, got {'ServiceCode': 'devicefarm', 'ServiceName': 'AWS Device Farm'}
$.Services[83]: expected {'ServiceCode': 'drs', 'ServiceName': 'ElasticDisasterRecovery'}, got {'ServiceCode': 'dlm', 'ServiceName': 'Amazon Data Lifecycle Manager'}
$.Services[89]: expected {'ServiceCode': 'ec2-ipam', 'ServiceName': 'IPAM'}, got {'ServiceCode': 'dsql', 'ServiceName': 'Amazon Aurora DSQL'}
$.Services[90]: expected {'ServiceCode': 'ec2fastlaunch', 'ServiceName': 'EC2 Fast Launch'}, got {'ServiceCode': 'dynamodb', 'ServiceName': 'Amazon DynamoDB'}
$.Services[94]: expected {'ServiceCode': 'eks-mcp', 'ServiceName': 'Amazon EKS MCP'}, got {'ServiceCode': 'ec2fastlaunch', 'ServiceName': 'Amazon EC2 Fast Launch'}
$.Services[98]: expected {'ServiceCode': 'elasticfilesystem', 'ServiceName': 'Amazon Elastic File System (EFS)'}, got {'ServiceCode': 'eks-mcp', 'ServiceName': 'eks-mcp'}
$.Services[101]: expected {'ServiceCode': 'elemental-inference', 'ServiceName': 'AWS Elemental Inference'}, got {'ServiceCode': 'elasticbeanstalk', 'ServiceName': 'AWS Elastic Beanstalk'}
$.Services[110]: expected {'ServiceCode': 'finspace', 'ServiceName': 'Amazon FinSpace'}, got {'ServiceCode': 'es', 'ServiceName': 'Amazon OpenSearch Service'}
$.Services[112]: expected {'ServiceCode': 'fis', 'ServiceName': 'AWS Fault Injection Service (AWS FIS)'}, got {'ServiceCode': 'evidently', 'ServiceName': 'Amazon CloudWatch Evidently'}
$.Services[119]: expected {'ServiceCode': 'geo', 'ServiceName': 'Amazon Location Service (Amazon Location)'}, got {'ServiceCode': 'fsx', 'ServiceName': 'Amazon FSx'}

Note the two independent symptoms visible together at indices 94 and 98: `ec2fastlaunch` is present but named 'Amazon EC2 Fast Launch' instead of AWS's 'EC2 Fast Launch', and `eks-mcp` is present but its ServiceName is the literal code 'eks-mcp' instead of 'Amazon EKS MCP'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay fixture `service-quotas-catalog-list-services`, step `task#1` (POST ServiceQuotasV20180901.ListServices).
3. The response is 200, but the `Services` array omits `deadline`, `dms`, `drs`, `ec2-ipam`, `elasticfilesystem`, `elemental-inference`, `finspace`, `fis`, `geo` and more, so every entry after index 40 is shifted; `eks-mcp` is also returned with ServiceName 'eks-mcp'.
4. The step must pass with the full AWS ServiceCode catalog returned in AWS's order, each with AWS's registered ServiceName.
