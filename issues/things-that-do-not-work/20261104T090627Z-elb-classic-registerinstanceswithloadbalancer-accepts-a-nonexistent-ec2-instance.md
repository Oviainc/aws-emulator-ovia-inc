Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elb.register-nonexistent-instance-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elb.register-nonexistent-instance-rejected|_error#5|AssertionError|raise AssertionError(
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# ELB Classic RegisterInstancesWithLoadBalancer accepts a nonexistent EC2 instance id with HTTP 200 instead of 400 InvalidInstance

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterInstancesWithLoadBalancer

## Description
Cause 1 of 1: RegisterInstancesWithLoadBalancer performs no existence check on the instance ids it is handed, so a well-formed but nonexistent instance id is registered successfully.

AWS: Smithy (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/smithy-model/com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7.json) models RegisterInstancesWithLoadBalancer with error InvalidEndPointException, errorKind client, httpError 400, awsQueryError code `InvalidInstance` (httpResponseCode 400) — documented as "The specified endpoint is not valid." The operation documentation states "The instance must be a running instance in the same network as the load balancer". Real AWS therefore refuses a registration naming an instance id that does not exist in the account/region: HTTP 400, Query error code `InvalidInstance`. Its message text embeds the request id of ELB's internal EC2 call and so is environment-specific (the fixed part is of the form `... Invalid id: "<instance-id>" (Service: AmazonEC2; ...; Request ID: <uuid>; Proxy: null)`, with `<instance-id>` taken from the request); the check asserts only the error CODE for this call, so the code and status are the requirement here. Nothing is registered by the refused call. The same InvalidEndPointException/`InvalidInstance` channel is modeled on DescribeInstanceHealth, where AWS answers 400 `InvalidInstance` with the fixed message `Could not find EC2 instance <instance-id>.` (the instance id is the value from the request) for an id that is not registered with that load balancer — the check asserts that too, but the replay aborted before reaching it, so the emulator's behavior there is unverified and only named as fix scope for this same missing-validation cause.

Emulator: the call returns HTTP 200 with the id in the registered set — replay observed `{'Instances': [{'InstanceId': 'i-0123456789abcdef0'}], ...'HTTPStatusCode': 200...}` for an instance id that was never created by the check (the id value itself is just this run's placeholder; the defect is that ANY nonexistent id is accepted). services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/register_instances_with_load_balancer.rs validates only that `Instances.member.InstanceId` is present and non-empty (lines 166-184), the batch size, the load balancer's existence and the per-load-balancer quota, then writes every requested id into the record (lines 215-220). No id is resolved against EC2 state, so no InvalidEndPointException is ever raised for a missing instance.

Why: a client that relies on AWS rejecting a bad registration sees it succeed. Phantom instances enter the load balancer's registration set, and every downstream read (DescribeInstanceHealth, DescribeLoadBalancers) reports backends that do not exist; error-handling paths and negative tests that expect `InvalidInstance` never fire, and typo'd or already-terminated instance ids are silently absorbed instead of surfaced.

Fix: the RegisterInstancesWithLoadBalancer handler's validation — resolve each requested instance id against the emulator's EC2 instance state before mutating the load balancer record, and return InvalidEndPointException (Query code `InvalidInstance`, HTTP 400) when any id does not resolve, registering none of the batch (the handler's own comment at lines 156-158 already states that all-or-nothing intent). Apply the same lookup to DescribeInstanceHealth's instance filter so an unregistered/nonexistent id yields 400 `InvalidInstance` with message `Could not find EC2 instance <instance-id>.`

Traces: elb.register-nonexistent-instance-rejected (mismatch; assertion `code == "InvalidInstance"` at check line 215/246 — the call AWS rejects succeeded).

## Evidence
Check elb.register-nonexistent-instance-rejected
Expected (AWS, per Smithy InvalidEndPointException / awsQueryError InvalidInstance): HTTP 400, Error.Code = "InvalidInstance", and the load balancer's registration set left empty.
Actual (emulator): HTTP 200 with body registering the id — AssertionError: "the call real AWS rejects SUCCEEDED here, returning \"{'Instances': [{'InstanceId': 'i-0123456789abcdef0'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}\""
Handler: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/register_instances_with_load_balancer.rs lines 166-184 (presence-only validation) and 215-220 (unconditional registration).

## Steps to Reproduce
1. Create a VPC with two subnets and a security group, and CreateLoadBalancer a Classic ELB in one of those subnets.
2. Call RegisterInstancesWithLoadBalancer(LoadBalancerName=<that LB>, Instances=[{InstanceId: "i-0123456789abcdef0"}]) — a well-formed instance id that no RunInstances in this account/region ever produced.
3. Observe HTTP 200 and the id echoed in Instances; AWS answers HTTP 400 with Error.Code "InvalidInstance" and registers nothing.
4. Follow with DescribeInstanceHealth(LoadBalancerName=<that LB>) and expect an empty InstanceStates list.
