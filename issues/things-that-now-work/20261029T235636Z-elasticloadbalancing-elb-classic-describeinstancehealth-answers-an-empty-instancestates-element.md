Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates
Issue-Related: 20261029T172824Z-elasticloadbalancing-elb-classic-describe-responses-serve-static-defaults-instead.md
Issue-Commit: beb9379647f6ddd981e0914c2206ccd791a0c1ce

# ELB Classic DescribeInstanceHealth answers an empty InstanceStates element instead of the registered instance's health state

> _Note: this issue was identified against the code base at commit `beb9379647f6ddd981e0914c2206ccd791a0c1ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceHealth

## Description
One cause. Cause 1 of 1: DescribeInstanceHealth serialises InstanceStates as an empty scalar element — no InstanceState member at all — for a load balancer that has an instance registered, and therefore never reports the unhealthy transition (fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection, step task#8).

AWS: Smithy: DescribeInstanceHealth (input DescribeEndPointStateInput {LoadBalancerName required, Instances}, output DescribeEndPointStateOutput {InstanceStates targeting the list InstanceStates of InstanceState}); errors AccessPointNotFoundException (awsQuery code LoadBalancerNotFound, 400) and InvalidEndPointException (awsQuery code InvalidInstance, 400). For a load balancer with registered instances the call answers 200 and InstanceStates carries ONE InstanceState per registered instance, serialised on this awsQuery/XML wire as repeated <member> elements (so a single registration parses as {'member': {...}}). InstanceState.InstanceId echoes the registered instance id; State has valid values InService | OutOfService | Unknown; ReasonCode has valid values ELB | Instance | N/A; Description is drawn from the service's enumerated sentence set, which includes exactly 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' An instance whose health checks fail UnhealthyThreshold times consecutively reports the triple State=OutOfService, ReasonCode=Instance, Description='Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' — fixed text, no substituted values; the only variable part of the member is the InstanceId, taken from the registration. The recording shows the registered instance present on every poll and that triple arriving on a later poll.

Emulator: the response is 200 but InstanceStates is an empty element — a scalar '' where a list is modelled: '$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates: type_mismatch (expected {member: {...}}, got \'\')'. There is no member, so the registered instance is not reported as being in ANY health state, and the Description assertion consequently never satisfies: 'poll: not in [\'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.\'] after 61 attempt(s) (got None)', with the 60s poll window exhausted ('$: poll capped at 60s').

Why: a client relying on AWS's behavior sees a load balancer that reports no instance health at all. SDK parsing of the response is broken before semantics even matter (a string arrives where the modelled list goes), so a caller iterating InstanceStates gets nothing; a deployment gate or wait helper that polls for InService or for OutOfService blocks until it times out; and an assertion of the form 'no instance is OutOfService' always passes, so unhealthy-instance handling (deregistration on failure, traffic draining) is never exercised. Hypothesis: the instance-health projection is built from a per-load-balancer registration set that the preceding steps of this fixture did not populate (or whose instance ids did not resolve), and the serializer writes an empty element rather than omitting the list or reporting the registered instances.

Fix: the DescribeInstanceHealth handler and the instance-health/registration state it reads in the elasticloadbalancing service — one InstanceState must be emitted per instance registered with the named load balancer (and per instance named in the request's Instances filter), serialised as repeated <member> elements of the modelled InstanceStates list rather than an empty scalar, with State/ReasonCode/Description read from stored per-instance health that can hold the OutOfService/Instance/failed-UnhealthyThreshold triple.

Related open issue that does NOT cover this: 20261029T172824Z-elasticloadbalancing-elb-classic-describe-responses-serve-static-defaults-instead.md carries this fixture and step, but its DescribeInstanceHealth keys are on InstanceStates.member.State / .ReasonCode / .Description — the frozen InService/N/A/N/A triple for an instance that IS reported. The divergence filed here is the InstanceStates list itself coming back as an empty scalar with no member, which that issue's keys do not cover and which the replay reports as unowned.

Traces: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#task#8.

## Evidence
Replay cluster (verdict mismatch, service elasticloadbalancing, operation DescribeInstanceHealth): expected_status 200, actual_status 200, divergence_key 'service=elasticloadbalancing|DescribeInstanceHealth|status=200->200|keys=$,$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates,$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.Description', affected ['elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#task#8'].
Detail, verbatim: '$: poll capped at 60s (expected {DescribeInstanceHealthResponse: {DescribeInstanceHealthResult: {InstanceStates: {member: {InstanceId: <registered instance id>, Description: \'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.\', State: \'OutOfService\', ReasonCode: \'Instance\'}}}, ResponseMetadata: {RequestId: <shaped-not-asserted>}}}, got {DescribeInstanceHealthResponse: {DescribeInstanceHealthResult: {InstanceStates: \'\'}, ResponseMetadata: {RequestId: <per-run value, not asserted>}}})'; '$...InstanceStates: type_mismatch (expected {member: {...}}, got \'\')'; '$...InstanceStates.member.Description: poll: not in [\'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.\'] after 61 attempt(s) (expected that sentence, got None)'.
Expected (AWS): 200 with InstanceStates carrying one InstanceState per registered instance; the unhealthy instance reports State 'OutOfService', ReasonCode 'Instance', Description 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' (all fixed text; InstanceId is the only value taken from the request/registration).
Actual (emulator): 200 with InstanceStates serialised as an empty scalar — no member — on all 61 polls.
Model evidence: DescribeInstanceHealth output DescribeEndPointStateOutput{InstanceStates -> list InstanceStates of InstanceState}; InstanceState.State valid values InService|OutOfService|Unknown; InstanceState.ReasonCode valid values ELB|Instance|N/A; InstanceState.Description enumerates the exact UnhealthyThreshold sentence; modeled errors AccessPointNotFoundException (LoadBalancerNotFound) and InvalidEndPointException (InvalidInstance).

## Steps to Reproduce
1. Replay fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection against the emulator up to step task#8: create a classic load balancer, register an instance with it, then poll elasticloadbalancing:DescribeInstanceHealth with that LoadBalancerName.
2. Expected: 200 and DescribeInstanceHealthResult.InstanceStates containing one <member> per registered instance, whose State/ReasonCode/Description eventually become OutOfService / Instance / 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.'
3. Actual: 200 with <InstanceStates/> serialised as an empty scalar on every poll — no member is returned, so the Description never appears and the 60s poll window is exhausted after 61 attempts.
4. Confirm a fix by re-replaying that fixture step: the list must parse as a list of InstanceState members and the registered instance must reach the OutOfService/Instance/UnhealthyThreshold triple within the poll window.
