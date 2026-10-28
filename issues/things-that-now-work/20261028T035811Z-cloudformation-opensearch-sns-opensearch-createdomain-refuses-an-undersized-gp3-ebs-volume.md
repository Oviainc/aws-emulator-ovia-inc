Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back
Issue-Key: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back|run#21|AssertionError|assert direct["code"] == "LimitExceededException", (
Issue-Commit: b46de20c42d1d13a6a9554983f2e031495be43a2

# OpenSearch CreateDomain refuses an undersized gp3 EBS volume with the wrong error class: 400 ValidationException instead of 409 LimitExceededException, and the refusal text does not name the instance type/engine bounds

> _Note: this issue was identified against the code base at commit `b46de20c42d1d13a6a9554983f2e031495be43a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause.

Cause 1 of 1: CreateDomain now does enforce the EBS volume-size floor, but it classifies the refusal as a request-shape error (ValidationException / HTTP 400) instead of the service-limit error AWS returns (LimitExceededException / HTTP 409), and it words the message differently from AWS.

Note on an existing open issue: 20261025T095029Z-cloudformation-opensearch-sns-opensearch-createdomain-ignores-the-ebs-minimumvolumesize-its-own.md covered the EARLIER form of this check's failure, where CreateDomain accepted the undersized volume and the stack reached CREATE_COMPLETE. Its replay key is the `assert status == "ROLLBACK_COMPLETE"` assertion, which now passes: the emulator refuses the call and the stack does roll back. The surviving failure is a different assertion (`assert direct["code"] == "LimitExceededException"`) and the suite does not mark this check as owned, so the misclassified refusal is filed here. No fixed issue claimed to fix this refusal.

AWS: Smithy (opensearch model, com-amazonaws-opensearch-amazonopensearchservice.json) binds CreateDomain to the distinct errors LimitExceededException (errorKind client, httpError 409) and ValidationException (errorKind client, httpError 400); LimitExceededException is documented as 'An exception for trying to create more than the allowed number of resources or sub-resources'. opensearch#StorageTypeLimit$LimitName documents MinimumVolumeSize and MaximumVolumeSize for StorageType 'ebs', and those are exactly the values DescribeInstanceTypeLimits returns in LimitsByRole[*].StorageTypes for a given InstanceType + EngineVersion. A CreateDomain whose EBSOptions.VolumeSize falls outside [MinimumVolumeSize, MaximumVolumeSize] for the requested instance type and engine is therefore a quota/limit refusal, not a shape refusal: recorded behavior is Error.Code 'LimitExceededException' with HTTP status 409 and the message 'Volume size must be between <min> and <max> for <instance-type> instance type and version <engine-version>'. In that message the FIXED text is 'Volume size must be between', ' and ', ' for ', ' instance type and version '; the VARIABLE parts are <min>/<max> (the bounds the service's own DescribeInstanceTypeLimits reports for that instance type and engine), <instance-type> (ClusterConfig.InstanceType from the request) and <engine-version> (EngineVersion from the request). CloudFormation then derives the resource handler's classification from that error class: the Domain CREATE_FAILED event's ResourceStatusReason quotes the service text as 'Resource handler returned message: "<that message> (Service: OpenSearch, Status Code: 409, Request ID: <id>) (SDK Attempt Count: 3)" (RequestToken: <id>, HandlerErrorCode: ServiceLimitExceeded)' — the HTTP status inside the reason is the same status the direct call returned, and HandlerErrorCode is ServiceLimitExceeded for a LimitExceededException.

Emulator: a direct CreateDomain with EBSOptions {EBSEnabled: true, VolumeType: 'gp3', VolumeSize: 5} on ClusterConfig.InstanceType 't3.small.search' is refused as {'code': 'ValidationException', 'message': 'EBSOptions.VolumeSize must be between 10 and 512 GiB for volume type gp3: 5', 'http_status': 400}. The error code, the HTTP status and the message all diverge: the emulator's text names the VOLUME TYPE and echoes the offending value with a ' GiB' unit, where AWS's names the INSTANCE TYPE and the ENGINE VERSION and omits the offending value. Because the class is wrong, the CloudFormation leg of the same scenario cannot be right either: a ValidationException maps to a non-ServiceLimitExceeded HandlerErrorCode and the 'Status Code:' embedded in the Domain CREATE_FAILED ResourceStatusReason reports 400 rather than 409.

Why: a client that branches on the error class gets the wrong answer. LimitExceededException is a service-quota condition (operators request a limit increase, SDK retry/classification treats it as a throttle-adjacent service condition); ValidationException is a malformed-request condition that is never retried and is surfaced to the caller as a bug in their request. Negative tests and infrastructure error handlers that assert LimitExceededException/409 pass against real AWS and fail against the emulator, and the HTTP status difference (400 vs 409) changes gateway/proxy and SDK retry behavior. In CloudFormation, deploy tooling that keys off HandlerErrorCode ServiceLimitExceeded to distinguish 'capacity/quota' failures from 'bad template' failures mis-attributes the rollback, and the status quoted inside ResourceStatusReason no longer matches what the service returns directly.

Fix: the EBS validation in CreateDomain's handler/validation in the OpenSearch service crate — services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs — must raise the modeled LimitExceededException (HTTP 409) rather than ValidationException when VolumeSize falls outside the per-instance-type/per-engine [MinimumVolumeSize, MaximumVolumeSize] bounds, and must word the message as AWS does, naming the bounds the service's own DescribeInstanceTypeLimits reports together with the request's instance type and engine version. UpdateDomainConfig's equivalent EBS check needs the same class and wording. The CloudFormation resource-handler error-class to HandlerErrorCode mapping must then yield ServiceLimitExceeded and quote the service's 409 in the Domain CREATE_FAILED ResourceStatusReason.

Traces: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back.

## Evidence
Expected (AWS): opensearch.create_domain(DomainName=<name>, EngineVersion=<engine>, ClusterConfig={'InstanceType': <itype>, ...}, EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': <below the reported MinimumVolumeSize>}) -> HTTP 409, Error.Code 'LimitExceededException', message 'Volume size must be between <min> and <max> for <itype> instance type and version <engine>' (fixed text as written; <min>/<max> from the service's own DescribeInstanceTypeLimits bounds, <itype>/<engine> echoed from the request), and no domain created.

Actual (emulator): HTTP 400, Error.Code 'ValidationException', message 'EBSOptions.VolumeSize must be between 10 and 512 GiB for volume type gp3: 5'.

  AssertionError: CreateDomain with gp3 VolumeSize=5 on t3.small.search must be refused with LimitExceededException; got {'code': 'ValidationException', 'message': 'EBSOptions.VolumeSize must be between 10 and 512 GiB for volume type gp3: 5', 'http_status': 400}
  (raised at check_cloudformation__opensearch__sns_deploy_opensearch_2_gp3_volume_below_instance_minimum_rejected_and_topic_rolled_back.py line 77, assert direct["code"] == "LimitExceededException")

Corroboration from the same check: the earlier legs now pass — the emulator's DescribeInstanceTypeLimits reports a gp3 MinimumVolumeSize strictly above the requested VolumeSize, and the CloudFormation stack does reach ROLLBACK_COMPLETE — so the floor IS enforced; only its error class, HTTP status and message text diverge. The assertions after line 77 (the message wording, and the Domain CREATE_FAILED ResourceStatusReason carrying '(Service: OpenSearch, Status Code: 409, ...)' with 'HandlerErrorCode: ServiceLimitExceeded') are downstream of the same classification and were not reached.

Smithy grounding: opensearch#CreateDomain errors include LimitExceededException (errorKind client, httpError 409) and ValidationException (errorKind client, httpError 400); opensearch#StorageTypeLimit$LimitName documents MinimumVolumeSize / MaximumVolumeSize for StorageType 'ebs'; opensearch#DescribeInstanceTypeLimits returns LimitsByRole -> Limits$StorageTypes.

## Steps to Reproduce
1. os = boto3.client('opensearch', endpoint_url=<emulator>)
2. Read the floor the emulator itself publishes:
   lim = os.describe_instance_type_limits(InstanceType='t3.small.search', EngineVersion='OpenSearch_2.11')
   -> the 'ebs' StorageType entry reports MinimumVolumeSize / MaximumVolumeSize (min is > 5).
3. Violate that floor directly:
   os.create_domain(DomainName='sim2real-ebs-floor', EngineVersion='OpenSearch_2.11',
                    ClusterConfig={'InstanceType': 't3.small.search', 'InstanceCount': 1,
                                   'DedicatedMasterEnabled': False, 'ZoneAwarenessEnabled': False},
                    EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': 5})
   Observed: ClientError with Code 'ValidationException', HTTP 400, message 'EBSOptions.VolumeSize must be between 10 and 512 GiB for volume type gp3: 5'.
   Expected: ClientError with Code 'LimitExceededException', HTTP 409, message 'Volume size must be between 10 and 512 for t3.small.search instance type and version OpenSearch_2.11' (bounds from step 2, instance type and engine echoed from the request).
4. CloudFormation leg: deploy a stack whose AWS::SNS::Topic is created first and whose AWS::OpenSearchService::Domain (DependsOn the topic) carries EBSOptions VolumeType gp3 / VolumeSize 5 on t3.small.search; then read DescribeStackEvents for the Domain's CREATE_FAILED event.
   Observed: the stack rolls back, but the ResourceStatusReason does not quote '(Service: OpenSearch, Status Code: 409, ...)' nor 'HandlerErrorCode: ServiceLimitExceeded', because the underlying refusal was a 400 ValidationException.
   Expected: ResourceStatusReason == 'Resource handler returned message: "Volume size must be between <min> and <max> for <itype> instance type and version <engine> (Service: OpenSearch, Status Code: 409, Request ID: <id>) (SDK Attempt Count: 3)" (RequestToken: <id>, HandlerErrorCode: ServiceLimitExceeded)'.
5. Replay: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back
