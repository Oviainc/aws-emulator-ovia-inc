Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back
Issue-Key: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back|run#17|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: ee3608f46372fd0499ccb380262b1cbf197ec697

# OpenSearch CreateDomain ignores the EBS MinimumVolumeSize its own DescribeInstanceTypeLimits reports: an undersized gp3 domain is created (200) instead of refused with LimitExceededException, so the CloudFormation stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `ee3608f46372fd0499ccb380262b1cbf197ec697`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause.

Cause 1 of 1: CreateDomain does not enforce the EBS volume-size bounds for the requested instance type and engine version, so a gp3 VolumeSize below the instance type's MinimumVolumeSize is accepted and the domain is materialized.

AWS: OpenSearch validates EBSOptions.VolumeSize against the per-instance-type/per-engine storage limits before creating a domain. Smithy: opensearch#CreateDomain takes EBSOptions (EBSEnabled/VolumeType/VolumeSize/Iops/Throughput) and models LimitExceededException with httpError 409 ('An exception for trying to create more than the allowed number of resources or sub-resources'); opensearch#StorageTypeLimit documents that for StorageType 'ebs' the limit names include MinimumVolumeSize ('Minimum volume size that is available for the given storage type') and MaximumVolumeSize, and these are exactly what DescribeInstanceTypeLimits returns in LimitsByRole[*].StorageTypes. So a CreateDomain whose VolumeSize is below the MinimumVolumeSize that DescribeInstanceTypeLimits reports for that InstanceType and EngineVersion is refused with LimitExceededException (HTTP 409) and creates no domain. The recorded refusal text is 'Volume size must be between <min> and <max> for <instance-type> instance type and version <engine-version>' — built from the request's instance type/engine and the service's own reported bounds, so the rule (name the bounds the service itself reports) is the requirement; the exact wording is an example. Downstream, CloudFormation turns that resource refusal into Domain CREATE_FAILED, stack ROLLBACK_IN_PROGRESS -> ROLLBACK_COMPLETE, and deletes the SNS topic the stack created first (recorded behavior for the CFN leg; HandlerErrorCode ServiceLimitExceeded).

Emulator: the emulator's own DescribeInstanceTypeLimits DOES report a MinimumVolumeSize above the requested size — the check's positive control on that premise passes — but CreateDomain then accepts the undersized EBSOptions anyway. Via CloudFormation, the AWS::OpenSearchService::Domain resource is created successfully and the stack ends 'CREATE_COMPLETE': 'AssertionError: a Domain that fails to create must roll the whole stack back; stack 'sim2real-xsvc-osrb-ebs-db94bd3a2799' ended 'CREATE_COMPLETE''. Because the resource never fails, nothing downstream is reachable either: there is no Domain CREATE_FAILED event, no ResourceStatusReason carrying the service refusal, no rollback, and the SNS topic is left in place.

Why: a client's storage-sizing negative test passes against the emulator and the same template/SDK call is refused by real AWS with a 409 LimitExceededException, so the configuration is not portable. The emulator is also internally inconsistent — it publishes a volume floor through DescribeInstanceTypeLimits and then violates it — so capacity-planning and validation code that reads the limits API cannot trust that the limits are enforced. Any deploy-time rollback/cleanup logic that depends on an undersized domain failing never runs, leaving resources (here an SNS topic) behind that AWS would have removed.

Fix: CreateDomain's EBSOptions validation in the OpenSearch service crate (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/create_domain.rs) — resolve the same per-instance-type/per-engine storage limits that DescribeInstanceTypeLimits serves and refuse VolumeSize outside [MinimumVolumeSize, MaximumVolumeSize] with LimitExceededException (HTTP 409), creating no domain. UpdateDomainConfig needs the same check. Note: the CloudFormation legs of this check (carrying the resource refusal into ResourceStatusReason, rolling the stack back and deleting the already-created topic) are separately covered by the open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md; they are only reachable once CreateDomain actually refuses. This cause is NOT one of the seven in the open issue 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md (which covers master-password complexity, OffPeakWindowOptions, ColdStorage/UltraWarm, LogPublishingOptions, DomainStatus empty members, service-software not-found and EC2/Service-Quotas support calls) — EBS volume-size bounds are not validated there.

Traces: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back.

## Evidence
Expected (AWS): opensearch.create_domain(DomainName=<name>, EngineVersion=<engine>, ClusterConfig={'InstanceType': <itype>, ...}, EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': <below MinimumVolumeSize>}) -> HTTP 409, Error.Code 'LimitExceededException', message naming the bounds the service itself reports (example: "Volume size must be between <min> and <max> for <itype> instance type and version <engine>"), and no domain created (DescribeDomain -> ResourceNotFoundException, ListDomainNames does not list it). The same Domain inside a CloudFormation stack -> Domain CREATE_FAILED with that text carried into ResourceStatusReason (HandlerErrorCode: ServiceLimitExceeded), stack ROLLBACK_COMPLETE, and the DependsOn SNS topic deleted.

Actual (emulator): CreateDomain succeeds and the domain is created; the CloudFormation stack reaches CREATE_COMPLETE.
  AssertionError: a Domain that fails to create must roll the whole stack back; stack 'sim2real-xsvc-osrb-ebs-db94bd3a2799' ended 'CREATE_COMPLETE'
  (raised at check_cloudformation__opensearch__sns_deploy_opensearch_2_gp3_volume_below_instance_minimum_rejected_and_topic_rolled_back.py line 77, assert status == "ROLLBACK_COMPLETE")

The check's preceding positive control passed, which is the key corroboration: the emulator's DescribeInstanceTypeLimits(InstanceType=<itype>, EngineVersion=<engine>) reported a gp3 MinimumVolumeSize strictly greater than the template's VolumeSize, so the floor the emulator publishes is the floor CreateDomain then ignores.

Smithy grounding: opensearch#CreateDomain errors include LimitExceededException (httpError 409); opensearch#StorageTypeLimit$LimitName documents MinimumVolumeSize / MaximumVolumeSize for StorageType 'ebs'; opensearch#DescribeInstanceTypeLimits returns LimitsByRole -> Limits$StorageTypes.

## Steps to Reproduce
1. os = boto3.client('opensearch', endpoint_url=<emulator>)
2. Read the floor the emulator itself publishes:
   lim = os.describe_instance_type_limits(InstanceType='t3.small.search', EngineVersion='OpenSearch_2.11')
   -> find the 'ebs' StorageType entry and its MinimumVolumeSize / MaximumVolumeSize LimitValues (min is > 5).
3. Call CreateDomain violating that floor:
   os.create_domain(DomainName='sim2real-ebs-floor', EngineVersion='OpenSearch_2.11',
                    ClusterConfig={'InstanceType': 't3.small.search', 'InstanceCount': 1,
                                   'DedicatedMasterEnabled': False, 'ZoneAwarenessEnabled': False},
                    EBSOptions={'EBSEnabled': True, 'VolumeType': 'gp3', 'VolumeSize': 5})
   Observed: HTTP 200 with a full DomainStatus. Expected: HTTP 409 LimitExceededException and no domain.
4. os.describe_domain(DomainName='sim2real-ebs-floor') -> succeeds (expected ResourceNotFoundException, 'Domain not found: sim2real-ebs-floor').
5. CloudFormation leg: deploy a stack whose AWS::SNS::Topic is created first and whose AWS::OpenSearchService::Domain (DependsOn the topic) carries EBSOptions VolumeType gp3 / VolumeSize 5 on t3.small.search, then poll DescribeStacks.
   Observed: StackStatus 'CREATE_COMPLETE', no Domain CREATE_FAILED event, topic still present (sns.get_topic_attributes succeeds).
   Expected: StackStatus 'ROLLBACK_COMPLETE', a Domain CREATE_FAILED event whose ResourceStatusReason carries the OpenSearch refusal text and 'HandlerErrorCode: ServiceLimitExceeded', and the topic deleted (sns.get_topic_attributes -> NotFound 'Topic does not exist').
6. Replay: cloudformation__opensearch__sns.deploy-opensearch-2-gp3-volume-below-instance-minimum-rejected-and-topic-rolled-back
