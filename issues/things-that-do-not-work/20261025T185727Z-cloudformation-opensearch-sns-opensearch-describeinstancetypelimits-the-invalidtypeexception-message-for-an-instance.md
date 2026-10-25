Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back
Issue-Key: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back|run#14|AssertionError|assert (on_engine["code"], on_engine["message"]) == ("InvalidTypeException", want_message), (
Issue-Commit: 5dcd1bd7969c2f584c7eb42c4a26285019dddcd0

# OpenSearch DescribeInstanceTypeLimits: the InvalidTypeException message for an instance type unsupported by the requested engine is not AWS's 'Invalid instance type: <type>' text (1 cause)

> _Note: this issue was identified against the code base at commit `5dcd1bd7969c2f584c7eb42c4a26285019dddcd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypeLimits

## Description
One root cause.

Cause 1 of 1: when an instance type is valid in general but not supported by the engine version named in the request, DescribeInstanceTypeLimits refuses with the right error shape but the wrong message text.
AWS: Smithy sns-adjacent model opensearch#DescribeInstanceTypeLimits (GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}, required httpLabel members InstanceType -> OpenSearchPartitionInstanceType enum and EngineVersion -> VersionString pattern ^Elasticsearch_[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$) models InvalidTypeException with errorKind client and httpError 409. For an engine-scoped instance-type refusal, AWS answers HTTP 409, Error.Code 'InvalidTypeException', and Error.Message 'Invalid instance type: <InstanceType>'. Fixed text: 'Invalid instance type: '; the single variable part is the InstanceType taken verbatim from the request path. The engine version is NOT named in the message. The same text is what CreateDomain returns when the same (type, engine) pair is rejected — the check asserts that identity next, so one message rule covers both operations.
Emulator: the code (InvalidTypeException) and status (409) are correct, but the message is a different sentence that names the engine as well: 'InstanceType <type> is not supported on <engine>'. Replay output: `AssertionError: DescribeInstanceTypeLimits('or1.medium.search', 'OpenSearch_2.3') must refuse the type for this engine; got {'code': 'InvalidTypeException', 'message': 'InstanceType or1.medium.search is not supported on OpenSearch_2.3', 'http_status': 409}`. The preceding positive control passed — the same type IS accepted for the newest 2.x engine — so the engine-scoping logic is right and only the refusal wording diverges.
Why: a client that matches on the refusal text (engine-upgrade planners, capacity selectors, deployment tooling that surfaces or classifies the OpenSearch refusal, and CloudFormation consumers that read the text carried into a Domain's ResourceStatusReason) sees a sentence real OpenSearch never emits, so the same negative path behaves differently against the emulator and against AWS. Because CreateDomain is specified to carry the identical text, any CFN ResourceStatusReason assertion downstream diverges too.
Fix: the InvalidTypeException construction in the OpenSearch service crate's DescribeInstanceTypeLimits handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_instance_type_limits.rs) and the same refusal in create_domain.rs / update_domain_config.rs — emit the message 'Invalid instance type: ' followed by the request's InstanceType, with HTTP 409 and Error.Code 'InvalidTypeException'. This cause is distinct from the open issue 20261025T095029Z-cloudformation-opensearch-sns-opensearch-createdomain-ignores-the-ebs-minimumvolumesize-its-own.md (EBS volume-size bounds not enforced by CreateDomain) and from 20261024T055421Z-opensearch-sns-sqs-opensearch-alerting-monitor-cannot-be-exercised-the-domain.md; neither touches the instance-type/engine refusal text.
Traces: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back.

## Evidence
Expected (AWS): opensearch.describe_instance_type_limits(InstanceType='or1.medium.search', EngineVersion='OpenSearch_2.3') -> HTTP 409, Error.Code 'InvalidTypeException', Error.Message 'Invalid instance type: or1.medium.search'. Fixed text 'Invalid instance type: '; variable part = the InstanceType from the request URI. The same (type, engine) pair on CreateDomain returns the same code and the same message.

Actual (emulator): HTTP 409 and Error.Code 'InvalidTypeException' (both correct), but Error.Message is 'InstanceType or1.medium.search is not supported on OpenSearch_2.3'.
  AssertionError: DescribeInstanceTypeLimits('or1.medium.search', 'OpenSearch_2.3') must refuse the type for this engine; got {'code': 'InvalidTypeException', 'message': 'InstanceType or1.medium.search is not supported on OpenSearch_2.3', 'http_status': 409}
  (raised at check_cloudformation__opensearch__sns_deploy_opensearch_2_instance_type_newer_than_engine_rejected_and_topic_rolled_back.py line 65, assert (on_engine["code"], on_engine["message"]) == ("InvalidTypeException", want_message))

Corroboration that the premise holds and only the wording diverges: the check's positive control immediately before this assertion passed — describe_instance_type_limits(InstanceType='or1.medium.search', EngineVersion=<newest OpenSearch 2.x the emulator reports>) returned LimitsByRole['data'], so the emulator does scope the type to the engine and does refuse the older engine; it just words the refusal differently.

Smithy grounding: opensearch#DescribeInstanceTypeLimits errors include InvalidTypeException (errorKind client, httpError 409) alongside BaseException 400, LimitExceededException 409, ResourceNotFoundException 409 and ValidationException 400; InstanceType and EngineVersion are required httpLabel members of the URI /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}.

## Steps to Reproduce
1. os = boto3.client('opensearch', endpoint_url=<emulator>, region_name='us-west-2')
2. Confirm the type is valid for a newer engine (positive control):
   os.describe_instance_type_limits(InstanceType='or1.medium.search', EngineVersion='<newest OpenSearch_2.x the emulator lists via list_versions>')
   -> 200 with LimitsByRole['data'] present.
3. Ask for the same type on an older engine that does not support it:
   try: os.describe_instance_type_limits(InstanceType='or1.medium.search', EngineVersion='OpenSearch_2.3')
   except ClientError as e: print(e.response['ResponseMetadata']['HTTPStatusCode'], e.response['Error']['Code'], repr(e.response['Error']['Message']))
   Observed: 409 InvalidTypeException 'InstanceType or1.medium.search is not supported on OpenSearch_2.3'
   Expected: 409 InvalidTypeException 'Invalid instance type: or1.medium.search'
4. Same pair through the create path, which must carry the identical text:
   os.create_domain(DomainName='sim2real-itype', EngineVersion='OpenSearch_2.3', ClusterConfig={'InstanceType': 'or1.medium.search', 'InstanceCount': 1}, EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': 20})
   Expected: InvalidTypeException with message 'Invalid instance type: or1.medium.search'.
5. Replay: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back
