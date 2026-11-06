Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__resourcegroupstaggingapi.load-balancer-and-target-group-tags-visible-through-get-resources elbv2__resourcegroupstaggingapi.tag-resources-nonexistent-target-group-rejected-per-resource
Issue-Key: elbv2__resourcegroupstaggingapi|CreateTargetGroup|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# Resource Groups Tagging over ELBv2 resources is unreachable: CreateTargetGroup answers InvalidAction at version 2015-12-01

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetGroup

## Description
One root cause, grounded in 2 failing behavioral checks whose id's leading token is the composite 'elbv2__resource-groups-tagging-api'. The already-open issue 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md names only CreateLoadBalancer under the 'acm__elbv2' token and does not carry these replay keys.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve CreateTargetGroup at API version 2015-12-01, so no taggable ELBv2 resource can be created and the tagging API has nothing to report on.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateTargetGroup (service version 2015-12-01, protocol awsQuery, arnNamespace 'elasticloadbalancing') accepts Tags (@length min 1) and returns {TargetGroups}; it is otherwise refused with one of its modeled awsQuery errors DuplicateTargetGroupName / InvalidConfigurationRequest / TooManyTargetGroups / TooManyTags, all HTTP 400. The resulting load balancer and target group ARNs are then discoverable through resourcegroupstaggingapi GetResources, and TagResources against a nonexistent target group ARN reports a per-resource failure rather than succeeding. 'InvalidAction' is reserved for an action the service does not have.
Emulator: both checks are refused on their first ELBv2 call with Error.Code 'InvalidAction' and message 'Could not find operation CreateTargetGroup for version 2015-12-01'.
Why: a client cannot verify that ELBv2 tags are visible through the account-wide tagging API, nor that tagging a fabricated target group ARN is reported as a per-resource failure — the cross-service tagging contract is entirely unobservable, and the SDK is told the ELBv2 action does not exist.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10, then ELBv2 tag persistence and its exposure to resourcegroupstaggingapi GetResources/TagResources.
Traces: elbv2__resource-groups-tagging-api.load-balancer-and-target-group-tags-visible-through-get-resources, elbv2__resource-groups-tagging-api.tag-resources-nonexistent-target-group-rejected-per-resource.

## Evidence
Both checks — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateTargetGroup operation: Could not find operation CreateTargetGroup for version 2015-12-01, raised before any resourcegroupstaggingapi call.
Expected: HTTP 200 with TargetGroups (tags applied), or HTTP 400 with a modeled awsQuery code.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').create_target_group(Name='sim2real-tg', Protocol='HTTP', Port=80, VpcId=<vpc>, Tags=[{'Key': 'sim2real', 'Value': 'yes'}]).
3. Observe Error.Code 'InvalidAction' / 'Could not find operation CreateTargetGroup for version 2015-12-01'; no ELBv2 ARN exists for resourcegroupstaggingapi.get_resources to return.
4. Re-replay both check ids in trace_ids.
