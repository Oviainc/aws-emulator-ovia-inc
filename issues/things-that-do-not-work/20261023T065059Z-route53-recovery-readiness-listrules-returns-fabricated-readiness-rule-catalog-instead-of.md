Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-readiness-catalog-list-rules
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-readiness-catalog-list-rules route53-recovery-readiness-list-catalog-operations
Issue-Shape-Id: com.amazonaws.route53recoveryreadiness#Route53RecoveryReadiness
Issue-Key: route53-recovery-readiness|ListRules|2xx->2xx|$.rules[*]
Issue-Commit: e2d4bffd2fe68cddce7016612710a92983157233

# ListRules returns fabricated readiness rule catalog instead of AWS rule set

> _Note: this issue was identified against the code base at commit `e2d4bffd2fe68cddce7016612710a92983157233`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRules

## Description
Cause 1 of 1: ListRules serves a readiness-rule catalog that the emulator invented, rather than the rule catalog Route53 Recovery Readiness actually publishes. This is ONE root cause (the seeded catalog data behind the ListRules handler) observed across 14 divergence clusters, one per `resourceType` filter value.

AWS: ListRules is modeled as `GET /rules` with an optional `resourceType` query member (Smithy: ListRules.ResourceType target `__string`, httpQuery `resourceType`; output ListRulesResponse.Rules -> list of ListRulesOutput with required members `ruleId`, `resourceType`, `ruleDescription`). The set of rules returned per resource type is recorded behavior only: the fixtures record AWS returning, per resource type, the full documented readiness-rule set keyed by stable `ruleId` values and the prose descriptions AWS ships with them -- e.g. for `AWS::ApiGateway::Stage`: `ApiGwV1ApiKeyCount` ("Inspects all API Gateway stages to ensure that they have same number of API Keys linked to them."), `ApiGwV1ApiKeySource`, `ApiGwV1BasePath`, `ApiGwV1BinaryMediaTypes`, `ApiGwV1CacheClusterEnabled`, `ApiGwV1CacheClusterSize`, `ApiGwV1CacheClusterStatus` and further `ApiGwV1*` rules (19 entries in the recording). The same shape of catalog is recorded for `AWS::ApiGatewayV2::Stage` (`ApiGwV2*`), `AWS::AutoScaling::AutoScalingGroup` (`AsgAZCount`, `AsgInstanceSizes`, `AsgInstanceTypes`, `AsgMinSizeAndMaxSize`, `AsgNormalizedCapacity`, `AsgQuotas`), `AWS::CloudWatch::Alarm` (`CloudWatchAlarmState`), `AWS::DynamoDB::Table` (`DynamoAutoScalingConfiguration`, `DynamoCapacity`, `DynamoConfiguration`, `DynamoGsiCapacity`, `DynamoGsiConfig`, ...), `AWS::EC2::CustomerGateway` (`CustomerGatewayIpAddress`, `CustomerGatewayState`, `CustomerGatewayVPNType`), `AWS::EC2::VPC` (`VpcCidrBlock`, `VpcCidrBlocksInAssociationSets`, ...), `AWS::EC2::VPNConnection` (`VpnConnectionsCategory`, ...), `AWS::EC2::VPNGateway` (`VpnGatewayAsn`, `VpnGatewayAttachment`, `VpnGatewayState`, `VpnGatewayType`), `AWS::EC2::Volume` (`EbsVolumeEncryption`, ...), `AWS::Lambda::Function` (`LambdaFunctionDeadLetterConfig`, ...), `AWS::MSK::Cluster` (`MskClusterACMCertificate`, ...), `AWS::RDS::DBCluster` (`RdsClusterStatus`, `RdsEngineMode`, `RdsEngineVersion`, `RdsGlobalReplicaLag`, `RdsInstanceType`, `RdsNormalizedCapacity`, `RdsQuotas`) and `AWS::SNS::Topic` (`SnsTopicAwsKmsKey`, `SnsTopicDeliveryPolicy`, `SnsTopicDisplayName`, `SnsTopicQuotas`, `SnsTopicSubscription`).

Emulator: answers 200 (status matches) but with a different, much smaller catalog whose `ruleId`s and `ruleDescription`s do not exist in the service -- they read like health-check assertions rather than readiness rules. Per resource type the emulator returns roughly two entries instead of the recorded set, e.g. `AWS::ApiGateway::Stage` -> `ApiGatewayStageDeployment` ("The API Gateway stage has an active deployment.") and `ApiGatewayStageThrottleQuota`; `AWS::AutoScaling::AutoScalingGroup` -> `AutoScalingGroupInServiceInstances`, `AutoScalingGroupMaxSizeQuota`; `AWS::CloudWatch::Alarm` -> `CloudWatchAlarmStateNotInAlarm` instead of `CloudWatchAlarmState`; `AWS::DynamoDB::Table` -> `DynamoDbTableCapacityQuota`, `DynamoDbTableReplicaActive`, `DynamoDbTableStatusActive`; `AWS::EC2::VPC` -> `VpcStateAvailable`, `VpcSubnetCapacity`; `AWS::RDS::DBCluster` -> `RdsDbClusterInstanceCount`, `RdsDbClusterStatusAvailable`; `AWS::SNS::Topic` -> `SnsTopicSubscriptionCount`. The replay reports every `$.rules[i]` as missing/extra, and the tail of each expected list has no counterpart at all (`got None`).

Why: ListRules is the discovery call clients use to learn which readiness rules exist before building resource sets and readiness checks, and `ruleId` is the stable identifier other Recovery Readiness surfaces key on. A client that enumerates rules against the emulator gets identifiers that do not exist in AWS and misses every real one, so rule-driven configuration, rule-coverage assertions and any mapping from `ruleId` to behavior silently targets nothing. Because the response is a 200, nothing surfaces the substitution: the client cannot tell it read an invented catalog.

Fix: the ListRules seed data / catalog backing the operation's handler in the route53recoveryreadiness service crate. The catalog must be replaced with the service's real rule set per resource type -- correct `ruleId` values, their AWS `ruleDescription` prose, and the full per-type cardinality -- not the health-assertion list currently seeded. The pagination contract (Smithy paginated: inputToken NextToken / outputToken NextToken / items Rules / pageSize MaxResults) should keep working over the corrected catalog.

Traces: fixture route53-recovery-readiness-catalog-list-rules, steps task#1 (AWS::ApiGateway::Stage), task#2 (AWS::ApiGatewayV2::Stage), task#3 (AWS::AutoScaling::AutoScalingGroup), task#4 (AWS::CloudWatch::Alarm, also affects route53-recovery-readiness-list-catalog-operations#task#5), task#5 and task#11 (AWS::DynamoDB::Table), task#6 (AWS::EC2::CustomerGateway), task#7 and task#16 (AWS::EC2::VPC), task#8/task#12/task#18 (AWS::EC2::VPNConnection), task#9 and task#17 (AWS::EC2::VPNGateway), task#10 and task#20 (AWS::EC2::Volume), task#13 (AWS::Lambda::Function), task#14 (AWS::MSK::Cluster), task#15 (AWS::RDS::DBCluster), task#19 (AWS::SNS::Topic). Smithy model consulted: services/service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness/smithy-model/com-amazonaws-route53recoveryreadiness-route53recoveryreadiness.json (ListRules, ListRulesOutput).

## Evidence
Expected vs actual, from the replay (verdict=mismatch, expected_status=200, actual_status=200 on every cluster -- the divergence is entirely in the body):

task#1 ($.rules[0]): expected {'ruleId': 'ApiGwV1ApiKeyCount', 'resourceType': 'AWS::ApiGateway::Stage', 'ruleDescription': 'Inspects all API Gateway stages to ensure that they have same number of API Keys linked to them.'}; got {'ruleId': 'ApiGatewayStageDeployment', 'resourceType': 'AWS::ApiGateway::Stage', 'ruleDescription': 'The API Gateway stage has an active deployment.'}. $.rules[2]..$.rules[18] expected ApiGwV1BasePath / ApiGwV1BinaryMediaTypes / ApiGwV1CacheClusterEnabled / ApiGwV1CacheClusterSize / ApiGwV1CacheClusterStatus ... but got None (list truncates after 2 entries).

task#3 ($.rules[0]): expected ruleId 'AsgAZCount'; got 'AutoScalingGroupInServiceInstances'. $.rules[2]..$.rules[5] expected AsgInstanceTypes / AsgMinSizeAndMaxSize / AsgNormalizedCapacity / AsgQuotas; got None.

task#4 ($.rules[0]): expected ruleId 'CloudWatchAlarmState' ("Inspects CloudWatch alarms to ensure that each is not in the ALARM or INSUFFICIENT_DATA state."); got 'CloudWatchAlarmStateNotInAlarm' ("The CloudWatch alarm is not in the ALARM state.").

task#15 ($.rules[0..6]): expected RdsClusterStatus / RdsEngineMode / RdsEngineVersion / RdsGlobalReplicaLag / RdsInstanceType / RdsNormalizedCapacity / RdsQuotas; got RdsDbClusterInstanceCount, RdsDbClusterStatusAvailable, then None.

task#19 ($.rules[0..4]): expected SnsTopicAwsKmsKey / SnsTopicDeliveryPolicy / SnsTopicDisplayName / SnsTopicQuotas / SnsTopicSubscription; got SnsTopicSubscriptionCount, then None.

No recorded `ruleId` the fixtures expect appears anywhere in the emulator's output, and no `ruleId` the emulator emits appears in any recording -- the two catalogs are disjoint. The rule descriptions quoted above are the AWS-published prose carried by the recording; the requirement is the rule identity and per-type completeness, with those descriptions as the recorded examples.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay fixture `route53-recovery-readiness-catalog-list-rules` (replay_wire_corpus()).
3. Step task#1 issues `GET /rules?resourceType=AWS::ApiGateway::Stage`. The response is 200 but `$.rules` contains `ApiGatewayStageDeployment` / `ApiGatewayStageThrottleQuota` instead of the recorded 19 `ApiGwV1*` rules.
4. Repeat for the other resourceType filters in the same fixture (task#2 ApiGatewayV2, task#3 AutoScaling, task#4 CloudWatch::Alarm, task#5 DynamoDB, task#6 EC2::CustomerGateway, task#7 EC2::VPC, task#8 EC2::VPNConnection, task#9 EC2::VPNGateway, task#10 EC2::Volume, task#13 Lambda::Function, task#14 MSK::Cluster, task#15 RDS::DBCluster, task#19 SNS::Topic) -- each returns 200 with a disjoint rule catalog.
5. Confirm the fix by re-replaying every step listed under Traces, including route53-recovery-readiness-list-catalog-operations#task#5 which the CloudWatch::Alarm cluster also covers.
