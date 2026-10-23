Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-trust-store-error-paths
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-trust-store-error-paths elbv2-trust-store-restricted-error-conditions elbv2-truststore-notfound-errors elbv2-truststore-revocation-full-workflow-not-found elbv2-trust-store-error-conditions elbv2-trust-store-revocation-error-conditions elbv2-trust-store-revocation-error-paths elbv2-truststore-not-found-errors elbv2-rule-target-group-load-balancer-not-found-errors elbv2-tag-management-lifecycle
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|AddTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|AddTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|AddTrustStoreRevocations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|RemoveTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|RemoveTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|RemoveTrustStoreRevocations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTrustStoreRevocations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTrustStoreRevocations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|GetTrustStoreCaCertificatesBundle|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|GetTrustStoreCaCertificatesBundle|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|GetTrustStoreCaCertificatesBundle|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|GetTrustStoreRevocationContent|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|GetTrustStoreRevocationContent|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|GetTrustStoreRevocationContent|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTargetHealth|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTargetHealth|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTargetHealth|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTags|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTags|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 trust store revocation, CA bundle, target health and tag actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTrustStoreRevocations

## Description
7 causes, one mechanism: the shared 'elasticloadbalancing' front door is the Classic ELB crate, which serves only Version=2012-06-01 and refuses every 2015-12-01 envelope with 400 InvalidAction 'Could not find operation <Action> for version <Version>' (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs); the ELBv2 model answers only under its private route key 'elasticloadbalancingv2'. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md, 20261023T025808Z-autoscaling-ecs-elbv2-elbv2-describetargethealth-is-not-dispatchable-at-version-2015.md (behavioral check ids only), 20261023T025808Z-elbv2-resource-groups-tagging-api-resource-groups-tagging-over-elbv2-resources-is-unreachable.md. Each cause needs the version dispatch fixed and then that operation's own handler; the recorded not-found messages are request-derived (they echo the ARN passed), so the rule is the requirement and the quoted text is an example.

Cause 1 of 7: AddTrustStoreRevocations on an absent trust store.
AWS: Smithy AddTrustStoreRevocations declares TrustStoreNotFoundException / awsQueryError 'TrustStoreNotFound' / 400 (alongside InvalidRevocationContent, RevocationContentNotFound, TooManyTrustStoreRevocationEntries).
Emulator: 400 Code 'InvalidAction', ErrorResponse.RequestId missing.
Why: CRL upload cannot run, and a client cannot tell a missing trust store from a missing API.
Fix: version dispatch, then AddTrustStoreRevocations' ARN lookup and revocation-content validation.
Traces: elbv2-trust-store-error-paths#task#8 (cluster of 4).

Cause 2 of 7: RemoveTrustStoreRevocations on an absent trust store.
AWS: TrustStoreNotFound / 400.
Emulator: 400 InvalidAction.
Why: revocation removal is unreachable.
Fix: version dispatch, then RemoveTrustStoreRevocations handler.
Traces: elbv2-trust-store-error-paths#task#9 (cluster of 4).

Cause 3 of 7: DescribeTrustStoreRevocations on an absent trust store.
AWS: TrustStoreNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: revocation listing/pagination is unreachable.
Fix: version dispatch, then DescribeTrustStoreRevocations handler.
Traces: elbv2-trust-store-error-conditions#task#3 (cluster of 6).

Cause 4 of 7: GetTrustStoreCaCertificatesBundle on an absent trust store.
AWS: Smithy GetTrustStoreCaCertificatesBundle -> {Location} with exactly one modeled error, TrustStoreNotFoundException / 'TrustStoreNotFound' / 400.
Emulator: 400 Code 'InvalidAction'.
Why: bundle retrieval is unreachable and its only modeled refusal never fires.
Fix: version dispatch, then GetTrustStoreCaCertificatesBundle handler.
Traces: elbv2-trust-store-error-paths#task#11 (cluster of 3).

Cause 5 of 7: GetTrustStoreRevocationContent on an absent trust store.
AWS: TrustStoreNotFound / 400.
Emulator: 400 InvalidAction.
Why: CRL content retrieval is unreachable.
Fix: version dispatch, then GetTrustStoreRevocationContent handler.
Traces: elbv2-trust-store-error-paths#task#12 (cluster of 3).

Cause 6 of 7: DescribeTargetHealth on an absent target group.
AWS: Smithy DescribeTargetHealth declares TargetGroupNotFoundException / 'TargetGroupNotFound' / 400 (plus InvalidTarget 400 and HealthUnavailable 500); message names the target group ARN.
Emulator: 400 Code 'InvalidAction'.
Why: no client can read target health or branch on a missing target group — the signal every ALB rollout check waits on.
Fix: version dispatch, then DescribeTargetHealth handler and its target-group lookup.
Traces: elbv2-rule-target-group-load-balancer-not-found-errors#task#4.

Cause 7 of 7: DescribeTags for an absent resource.
AWS: Smithy DescribeTags(ResourceArns required) declares LoadBalancerNotFound, TargetGroupNotFound, ListenerNotFound, RuleNotFound and TrustStoreNotFound — all awsQuery 400; a fabricated target-group ARN is TargetGroupNotFound with a message naming it.
Emulator: 400 Code 'InvalidAction'.
Why: tag reads over ELBv2 resources are unreachable, and tag-based tooling cannot tell absence from a dead API.
Fix: version dispatch, then DescribeTags' ARN-kind resolution and per-kind not-found errors.
Traces: elbv2-tag-management-lifecycle#task#7.

## Evidence
All actual responses are HTTP 400 with $.ErrorResponse.Error.Code 'InvalidAction' and Message 'Could not find operation <Action> for version 2015-12-01', and $.ErrorResponse.RequestId missing.
- elbv2-trust-store-error-paths#task#8 AddTrustStoreRevocations, #task#9 RemoveTrustStoreRevocations, #task#11 GetTrustStoreCaCertificatesBundle, #task#12 GetTrustStoreRevocationContent and elbv2-trust-store-error-conditions#task#3 DescribeTrustStoreRevocations: expected Code 'TrustStoreNotFound' with a message naming the trust store ARN (example "Trust store ARN 'arn:aws:elasticloadbalancing:us-west-2:000000000000:truststore/sim2real-probe-elbv2-t-72d87a6c/0123456789abcdef' not found"); got 'InvalidAction'.
- elbv2-rule-target-group-load-balancer-not-found-errors#task#4 DescribeTargetHealth: expected 'TargetGroupNotFound' naming the target-group ARN; got 'InvalidAction'.
- elbv2-tag-management-lifecycle#task#7 DescribeTags: expected 'TargetGroupNotFound' naming the ARN; got 'InvalidAction'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Signing for service 'elasticloadbalancing', POST 'Action=AddTrustStoreRevocations&Version=2015-12-01&TrustStoreArn=arn:aws:elasticloadbalancing:us-west-2:000000000000:truststore/absent/0123456789abcdef'. Observe 400 InvalidAction instead of 400 TrustStoreNotFound.
3. Repeat with Action = RemoveTrustStoreRevocations / DescribeTrustStoreRevocations / GetTrustStoreCaCertificatesBundle / GetTrustStoreRevocationContent against the same ARN, with Action=DescribeTargetHealth against a fabricated target-group ARN (expect TargetGroupNotFound), and with Action=DescribeTags&ResourceArns.member.1=<fabricated target-group ARN> (expect TargetGroupNotFound).
4. Re-replay the fixture steps named in issue_step and trace_ids.
