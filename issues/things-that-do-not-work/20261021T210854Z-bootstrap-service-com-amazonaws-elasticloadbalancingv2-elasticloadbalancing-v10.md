Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Elastic Load Balancing v2 emulator

_Filed: 2026-10-21T21:08:54Z_
_Operation: service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 51 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and the service holds no state. Implement a FIRST, SMALL, stateful slice: the core ELBv2 provisioning loop for an Application Load Balancer, so a realistic client (aws CLI / boto3 `elbv2`) can create a load balancer, give it a target group and listener, register targets, read it back, and tear it down.

Wire protocol is `awsQuery` (sigv4 name `elasticloadbalancing`), so responses must be serialized as `<OperationName>Response`/`<OperationName>Result` XML by the framework — handlers only build the modeled output structs.

### Happy path to implement (in order)

1. **CreateLoadBalancer** — input `Name` (required), `Subnets`/`SubnetMappings`, `SecurityGroups`, `Scheme` (`internal`|`internet-facing`, default `internet-facing`), `Type` (`application`|`network`|`gateway`, default `application`), `IpAddressType`, `Tags`. Must mint and persist a load balancer and return `LoadBalancers: [LoadBalancer{LoadBalancerArn, LoadBalancerName, DNSName, CanonicalHostedZoneId, CreatedTime, Scheme, VpcId, State{Code: "provisioning"|"active"}, Type, AvailabilityZones, SecurityGroups, IpAddressType}]`. ARN shape `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<16-hex>`; DNS name `<name>-<id>.<region>.elb.amazonaws.com`. A second create with the same `Name` must fail with `DuplicateLoadBalancerNameException` (HTTP 400).
2. **CreateTargetGroup** — input `Name` (required), `Protocol`, `Port`, `VpcId`, `TargetType` (default `instance`), health-check fields. Persist and return `TargetGroups: [TargetGroup{TargetGroupArn, TargetGroupName, Protocol, Port, VpcId, TargetType, HealthCheckEnabled, HealthCheckProtocol, HealthCheckPort, HealthCheckPath, HealthCheckIntervalSeconds, HealthCheckTimeoutSeconds, HealthyThresholdCount, UnhealthyThresholdCount, Matcher, LoadBalancerArns: []}]` with defaults materialized (e.g. interval 30, timeout 5, thresholds 5/2, matcher `200` for HTTP). Duplicate name → `DuplicateTargetGroupNameException`.
3. **CreateListener** — input `LoadBalancerArn` (required), `Protocol`, `Port`, `DefaultActions` (required), `Certificates`, `SslPolicy`. Must look the load balancer up in state and return `ListenerNotFoundException`/`LoadBalancerNotFoundException` when absent; persist the listener under its load balancer and return `Listeners: [Listener{ListenerArn, LoadBalancerArn, Port, Protocol, DefaultActions, SslPolicy, Certificates}]`. A `forward` default action must record the referenced `TargetGroupArn` on the target group's `LoadBalancerArns` (unknown ARN → `TargetGroupNotFoundException`).
4. **RegisterTargets** — input `TargetGroupArn` (required), `Targets` (required, `TargetDescription{Id, Port, AvailabilityZone}`). Must store the targets on the persisted target group (`TargetGroupNotFoundException` for an unknown ARN) and return an empty success. **DescribeTargetHealth** (input `TargetGroupArn`, optional `Targets`) must then return one `TargetHealthDescription{Target, HealthCheckPort, TargetHealth{State, Reason, Description}}` per registered target, derived from the stored registrations — not a canned list.
5. **DescribeLoadBalancers** — input `LoadBalancerArns` / `Names` / `Marker` / `PageSize`. Must return exactly the load balancers in state, filtered by the supplied ARNs or names, each field byte-identical to what `CreateLoadBalancer` returned; an unknown ARN or name → `LoadBalancerNotFoundException`. With no filter, return all. Honor `PageSize` by emitting `NextMarker` and resuming from it (`@paginated(inputToken=Marker, outputToken=NextMarker, items=LoadBalancers)`).
6. **DeleteLoadBalancer** — input `LoadBalancerArn` (required). Must remove the load balancer and its listeners from state (`LoadBalancerNotFoundException` if absent) and return an empty success; a subsequent `DescribeLoadBalancers` on that ARN must then raise `LoadBalancerNotFoundException`.

Every one of these must compute its response from persisted state. Hardcoded responses, echoing the input back, or minting an ARN without storing anything are failures: a create followed by a describe must return what was created, and a delete must be observable by the next describe.

## State Model

In `src/state.rs`, on the single shared engine (reached via `crate::state::service_state(&*self.engine)`):

- `load_balancers: HashMap<String /*lb arn*/, LoadBalancerRecord>` where `LoadBalancerRecord { arn, name, dns_name, canonical_hosted_zone_id, created_time, scheme, vpc_id, state_code, lb_type, availability_zones: Vec<AvailabilityZone{zone_name, subnet_id, load_balancer_addresses}>, security_groups: Vec<String>, ip_address_type, attributes: HashMap<String,String>, tags: HashMap<String,String>, listener_arns: Vec<String> }`
- `name_to_lb_arn: HashMap<String, String>` to enforce `DuplicateLoadBalancerNameException` and serve `Names` lookups.
- `target_groups: HashMap<String /*tg arn*/, TargetGroupRecord>` where `TargetGroupRecord { arn, name, protocol, protocol_version, port, vpc_id, target_type, health_check: HealthCheckConfig{enabled, protocol, port, path, interval_seconds, timeout_seconds, healthy_threshold, unhealthy_threshold, matcher}, load_balancer_arns: Vec<String>, attributes: HashMap<String,String>, tags: HashMap<String,String>, targets: HashMap<TargetKey(id, port), RegisteredTarget{id, port, availability_zone, health_state, reason, description, registered_at}> }`
- `name_to_tg_arn: HashMap<String, String>` for `DuplicateTargetGroupNameException` and `Names` filtering.
- `listeners: HashMap<String /*listener arn*/, ListenerRecord>` where `ListenerRecord { arn, load_balancer_arn, port, protocol, ssl_policy, certificates: Vec<Certificate>, default_actions: Vec<Action>, alpn_policy, attributes, tags, rule_arns: Vec<String> }`
- Counters/ids for ARN minting (`next_id` or a random 16-hex suffix generator) kept in the same guarded struct so ARNs are stable and unique within a run.

All of the above live inside the one lockable state value so every handler in the crate sees the same store across requests.

## Out of scope for this slice

Rules (`CreateRule`/`DescribeRules`/`ModifyRule`/`SetRulePriorities`), trust stores, certificates, capacity reservations, SSL policies, tagging ops, attribute modify/describe ops, `SetSubnets`/`SetSecurityGroups`/`SetIpAddressType`, `DescribeAccountLimits`, and the other ~40 operations stay at HTTP 501 and will be filed as individual gap issues later. The state model above is deliberately shaped so they can be layered on without a rewrite.

## Evidence

OPERATIONS.md (crate root) lists 51 operations with protocol `awsQuery`, sdkId `Elastic Load Balancing v2`; it gives the exact inputs/outputs/errors cited above for CreateLoadBalancer, CreateTargetGroup, CreateListener, RegisterTargets, DescribeTargetHealth, DescribeLoadBalancers and DeleteLoadBalancer, including the `@paginated(inputToken=Marker, outputToken=NextMarker, items=LoadBalancers)` trait and the `DuplicateLoadBalancerNameException` / `DuplicateTargetGroupNameException` / `LoadBalancerNotFoundException` / `TargetGroupNotFoundException` / `ListenerNotFoundException` errors (all HTTP 400). Smithy shapes `LoadBalancer`, `TargetGroup`, `Listener` and `TargetHealthDescription` supply the response member names used in the state model. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 51 handler files (e.g. src/ops/create_load_balancer.rs:4, src/ops/describe_load_balancers.rs:4, src/ops/register_targets.rs:4), proving there is no implementation and no persisted state today.

## Steps to Reproduce

1. Start the emulator and point an elbv2 client at it.
2. CreateLoadBalancer Name=my-alb Subnets=subnet-1,subnet-2 Type=application Scheme=internet-facing -> expect a LoadBalancer with a loadbalancer/app/my-alb/<id> ARN, DNSName, CreatedTime and State; today: HTTP 501 NotImplemented.
3. CreateLoadBalancer Name=my-alb again -> expect HTTP 400 DuplicateLoadBalancerNameException.
4. CreateTargetGroup Name=my-tg Protocol=HTTP Port=80 VpcId=vpc-1 TargetType=instance -> expect a TargetGroup with minted ARN and defaulted health-check fields.
5. CreateListener LoadBalancerArn=<lb arn> Protocol=HTTP Port=80 DefaultActions=[{Type=forward,TargetGroupArn=<tg arn>}] -> expect a Listener echoing the stored config with a minted listener ARN.
6. RegisterTargets TargetGroupArn=<tg arn> Targets=[{Id=i-1111111111111111,Port=80}] -> expect empty success, then DescribeTargetHealth TargetGroupArn=<tg arn> -> expect exactly that one target with a TargetHealth state.
7. DescribeLoadBalancers (no filter) -> expect the created load balancer; DescribeLoadBalancers Names=my-alb -> same record; DescribeLoadBalancers LoadBalancerArns=<bogus arn> -> HTTP 400 LoadBalancerNotFoundException.
8. DeleteLoadBalancer LoadBalancerArn=<lb arn> -> empty success; DescribeLoadBalancers LoadBalancerArns=<lb arn> -> HTTP 400 LoadBalancerNotFoundException.
