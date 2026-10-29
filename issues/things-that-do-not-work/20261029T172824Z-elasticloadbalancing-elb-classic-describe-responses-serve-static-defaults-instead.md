Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.Description
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.ReasonCode
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.State
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->2xx|$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.Policies.OtherPolicies
Issue-Commit: 47f32ebbccbde7752714345af879a016dd1bff94

# ELB Classic describe responses serve static defaults instead of tracked load balancer state

> _Note: this issue was identified against the code base at commit `47f32ebbccbde7752714345af879a016dd1bff94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceHealth

## Description
Two ELB Classic read operations answer 200 with hard-coded placeholder content rather than serialising the state the preceding mutations put into the emulator, so a client polling or inspecting a load balancer never observes the effect of its own calls. 2 causes.

Cause 1 of 2: DescribeInstanceHealth never reports an unhealthy instance (fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection, step task#8).
AWS: Smithy: DescribeInstanceHealth (input DescribeEndPointStateInput {LoadBalancerName required, Instances}, output DescribeEndPointStateOutput {InstanceStates}); shape InstanceState members State ("Valid values: InService | OutOfService | Unknown"), ReasonCode ("Valid values: ELB | Instance | N/A" and documented as information about the cause of OutOfService instances) and Description (an enumerated set of sentences, one of which is exactly "Instance has failed at least the UnhealthyThreshold number of health checks consecutively."). An instance registered with a classic load balancer whose health checks fail UnhealthyThreshold times consecutively transitions to State=OutOfService with ReasonCode=Instance and that Description; the three members move together as one state, and the InstanceId echoes the registered instance from the request/registration. The recording shows exactly that transition arriving on a later poll of DescribeInstanceHealth.
Emulator: the instance stays State=InService, ReasonCode=N/A, Description=N/A forever. The fixture polled the operation 61 times over the 60s cap and the member never left the InService triple, so all three members diverge on every poll (expected State 'OutOfService' / ReasonCode 'Instance' / Description 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.', got 'InService' / 'N/A' / 'N/A'). The InstanceId i-0000... matched, so the instance IS registered in state; only its health is frozen at the registration default.
Why: a client that registers an instance and waits for it to be marked unhealthy (deregistration-on-failure logic, health gates in deploy scripts, any wait/poll helper built on DescribeInstanceHealth) blocks forever and then times out, and a client that asserts "no instance is OutOfService" always passes, so unhealthy-instance handling is never exercised. Hypothesis: the health field is a constant returned at serialisation time rather than a per-instance value kept in state.
Fix: the DescribeInstanceHealth handler plus the instance-health state it reads — instance health must be stored per registered instance and must be able to hold the OutOfService/Instance/failed-UnhealthyThreshold triple, with State, ReasonCode and Description serialised from that stored value instead of from constants.
Traces: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#task#8.

Cause 2 of 2: DescribeLoadBalancers serialises Policies.OtherPolicies as an empty scalar instead of the created policy names (fixture elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy, step task#12).
AWS: Smithy: DescribeLoadBalancers (output DescribeAccessPointsOutput {LoadBalancerDescriptions}); LoadBalancerDescription.Policies is shape Policies with member OtherPolicies targeting the list PolicyNames, documented as "The policies other than the stickiness policies." After a non-stickiness policy is created on the load balancer, DescribeLoadBalancers reports its name in Policies.OtherPolicies, which on this awsQuery/XML wire is a list serialised as repeated <member> elements (so the parsed shape is {"member": [...]}, or a single "member" value for one policy name). The policy name is whatever the client passed to CreateLoadBalancerPolicy.
Emulator: the element is serialised as an empty scalar — `$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.Policies.OtherPolicies: type_mismatch (expected {'member': <policy name>}, got '')`. The expected member value in this recording is the generated probe policy name; the requirement is that the name the client created appears, not that literal string.
Why: the response says the load balancer carries no non-stickiness policies right after one was created, so a client that reads back its policy set to verify creation, to enumerate policies before deleting them, or to decide whether to re-apply a policy sees nothing and either errors or loops re-creating. The empty scalar also breaks SDK list parsing (a string where a list is modelled) rather than merely being empty.
Fix: the DescribeLoadBalancers serializer and the load balancer's policy state — OtherPolicies must be populated from the policies attached to the load balancer and emitted as a PolicyNames list (repeated <member>), omitted when genuinely empty rather than written as an empty element.
Traces: elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy#task#12.

## Evidence
Cause 1 — DescribeInstanceHealth, expected 200 / actual 200: expected InstanceStates.member = {InstanceId: i-00000000000000001, State: 'OutOfService', ReasonCode: 'Instance', Description: 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.'}; actual = {InstanceId: i-00000000000000001, State: 'InService', ReasonCode: 'N/A', Description: 'N/A'}. Replay detail: "poll: not in ['Instance has failed at least the UnhealthyThreshold number of health checks consecutively.'] after 61 attempt(s)"; "$: poll capped at 60s".
Cause 2 — DescribeLoadBalancers, expected 200 / actual 200: "$.DescribeLoadBalancersResponse.DescribeLoadBalancersResult.LoadBalancerDescriptions.member.Policies.OtherPolicies: type_mismatch (expected {'member': 'sim2real-probe-pol-proxy-52b7faf1'}, got '')" — a list of policy names was expected, an empty scalar was returned.
Model evidence: Smithy InstanceState.State valid values InService|OutOfService|Unknown; InstanceState.ReasonCode valid values ELB|Instance|N/A; InstanceState.Description enumerates the exact UnhealthyThreshold sentence; Policies.OtherPolicies targets list PolicyNames.

## Steps to Reproduce
1. Replay fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection against the emulator and stop at step task#8: create a classic load balancer, register an instance whose health checks fail, then poll DescribeInstanceHealth. The member stays State=InService/ReasonCode=N/A/Description=N/A for the full 60s poll window instead of becoming OutOfService/Instance/'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.'.
2. Replay fixture elbclassic-policy-lifecycle-cookie-stickiness-and-generic-policy and stop at step task#12: create a load balancer, create a non-stickiness load balancer policy on it, then call DescribeLoadBalancers. LoadBalancerDescriptions.member.Policies.OtherPolicies comes back as an empty element instead of a list containing the policy name just created.
