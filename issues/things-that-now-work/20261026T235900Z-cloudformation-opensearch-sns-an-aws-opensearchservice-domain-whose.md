Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back
Issue-Key: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back|run#24|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation__opensearch__sns: an AWS::OpenSearchService::Domain whose instance type the requested engine does not support is created instead of failing, so the stack reaches CREATE_COMPLETE and nothing rolls back

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — CreateDomain does not enforce instance-type/engine-version compatibility, so a template that must fail its Domain resource completes the whole stack.
AWS: recorded behavior only — a Domain declaring an instance type the chosen OpenSearch 2.x engine version does not support is refused by OpenSearch with its own InvalidTypeException; CloudFormation therefore fails that resource, carries the service's refusal text into the resource's ResourceStatusReason (HandlerErrorCode InvalidRequest), and rolls the stack back — the SNS topic created first (via DependsOn) is deleted, so the stack ends ROLLBACK_COMPLETE and neither resource survives. The same instance type IS accepted for the newest supported 2.x engine, so the refusal is engine-scoped rather than a blanket rejection.
Emulator: the stack ends `CREATE_COMPLETE`: the Domain is created with the unsupported instance type, no resource fails, and no rollback happens (the DependsOn topic survives too).
Why: an invalid cluster topology is accepted as a successful deployment, so the caller believes it has a working domain on a type that cannot exist, and the entire rollback contract (service refusal -> resource failure -> reverse-order teardown) is never exercised. Related already-filed work does not cover this: open issue 20261025T095029Z-cloudformation-opensearch-sns-opensearch-createdomain-ignores-the-ebs-minimumvolumesize-its-own.md is the EBS MinimumVolumeSize rule, and fixed issue 20261025T185727Z-cloudformation-opensearch-sns-opensearch-describeinstancetypelimits-... only corrected the InvalidTypeException message on DescribeInstanceTypeLimits — CreateDomain itself still accepts the pairing.
Fix: the opensearch CreateDomain handler's validation (instance type must be checked against the requested EngineVersion, refusing with InvalidTypeException), and the AWS::OpenSearchService::Domain provisioner must propagate that refusal as the resource's failure so CloudFormation rolls back.
Traces: cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back

## Evidence
cloudformation__opensearch__sns.deploy-opensearch-2-instance-type-newer-than-engine-rejected-and-topic-rolled-back:
  assert status == "ROLLBACK_COMPLETE"
  AssertionError: a Domain that fails to create must roll the whole stack back; stack 'sim2real-xsvc-osrb-itype-<id>' ended 'CREATE_COMPLETE'

Template: deployments/domain-instance-type-newer-than-engine.json — an AWS::OpenSearchService::Domain on an instance type newer than its 2.x EngineVersion, with an AWS::SNS::Topic created first via DependsOn; the check then verifies SNS and OpenSearch agree that neither resource survived.

## Steps to Reproduce
1. CreateStack with a template containing an AWS::SNS::Topic and an AWS::OpenSearchService::Domain (DependsOn the topic) whose ClusterConfig.InstanceType is newer than the Domain's declared 2.x EngineVersion.
2. Poll DescribeStacks to a terminal status: expect ROLLBACK_COMPLETE, the Domain resource FAILED with the service's InvalidTypeException text in ResourceStatusReason, and the topic deleted. The emulator reports CREATE_COMPLETE with both resources present.
3. Control: the same instance type with the newest supported 2.x engine must still deploy successfully.
