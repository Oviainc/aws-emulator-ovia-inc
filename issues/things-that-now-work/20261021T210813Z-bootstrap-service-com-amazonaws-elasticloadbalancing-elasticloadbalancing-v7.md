Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Elastic Load Balancing (Classic ELB) emulator

_Filed: 2026-10-21T21:08:13Z_
_Operation: service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 29 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the stateful create -> use -> read -> delete lifecycle of the primary resource, the Classic load balancer (`AccessPoint`).

Happy path to implement, in order (awsQuery protocol, sigv4 signing name `elasticloadbalancing`):

1. **CreateLoadBalancer** — input `LoadBalancerName` (required), `Listeners` (required, list<Listener> of `Protocol`/`LoadBalancerPort`/`InstanceProtocol`/`InstancePort`/`SSLCertificateId`), `AvailabilityZones`, `Subnets`, `SecurityGroups`, `Scheme`, `Tags`. Must persist a new load balancer record keyed by name and return a generated, deterministic `DNSName` (e.g. `<name>-<account-ish-suffix>.<region>.elb.amazonaws.com`) that the same record keeps for its lifetime. A second create with the same name must fail with `DuplicateAccessPointNameException` (HTTP 400), not succeed.
2. **RegisterInstancesWithLoadBalancer** — input `LoadBalancerName`, `Instances` (list<Instance> of `InstanceId`). Must add the instance IDs to the stored load balancer (deduplicated) and return `Instances` = the FULL current registered set read back from state, not just the ones in the request. Unknown `LoadBalancerName` => `AccessPointNotFoundException` (HTTP 400).
3. **DescribeLoadBalancers** — input `LoadBalancerNames`, `Marker`, `PageSize` (1..400); output `LoadBalancerDescriptions`, `NextMarker`. Must project each stored record into a `LoadBalancerDescription` with at minimum `LoadBalancerName`, `DNSName`, `CanonicalHostedZoneName`, `CanonicalHostedZoneNameID`, `ListenerDescriptions` (the listeners exactly as created, each wrapped with its `PolicyNames`), `AvailabilityZones`, `Subnets`, `SecurityGroups`, `Instances`, `HealthCheck`, `Scheme`, `CreatedTime`. Filtering by `LoadBalancerNames` must work, an unknown name in that list must raise `AccessPointNotFoundException`, and `PageSize`/`Marker` must actually bound and resume the page (op is `@paginated(inputToken=Marker, outputToken=NextMarker, items=LoadBalancerDescriptions)`).
4. **DescribeInstanceHealth** — input `LoadBalancerName`, optional `Instances`; output `InstanceStates` (list<InstanceState> of `InstanceId`, `State`, `ReasonCode`, `Description`). Must derive one entry per registered instance from state (a registered instance may report `InService` with `ReasonCode` `N/A`); an instance ID that is NOT registered with the load balancer must report `OutOfService` / `ReasonCode` `ELB` / description `Instance is not currently registered with the LoadBalancer.` rather than being omitted or erroring. Unknown load balancer => `AccessPointNotFoundException`.
5. **DeregisterInstancesFromLoadBalancer** — input `LoadBalancerName`, `Instances`; must remove those IDs from the stored record and return the remaining registered `Instances` read back from state. A following DescribeInstanceHealth/DescribeLoadBalancers must no longer list them.
6. **DeleteLoadBalancer** — input `LoadBalancerName`, no output members, no modeled errors. Must remove the record (and its listeners/instances/tags) and be idempotent: deleting a name that does not exist returns a successful empty response. After delete, DescribeLoadBalancers with that name must raise `AccessPointNotFoundException` and an unfiltered DescribeLoadBalancers must not list it.

No canned or echoed responses: every one of these six must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (the shared engine), so a create is observable by a later describe in the same process. Delete each handler's `//! SCAFFOLDED, NOT IMPLEMENTED:` marker as it is implemented.

## State Model

In `src/state.rs`, on the service state struct:

- `load_balancers: BTreeMap<String /*LoadBalancerName*/, LoadBalancer>` — BTreeMap so DescribeLoadBalancers has a stable order for `Marker`-based pagination.
- `LoadBalancer { name: String, dns_name: String, canonical_hosted_zone_name: String, canonical_hosted_zone_name_id: String, scheme: String /*default "internet-facing"*/, created_time: SystemTime, listeners: Vec<ListenerEntry>, availability_zones: Vec<String>, subnets: Vec<String>, security_groups: Vec<String>, vpc_id: Option<String>, instances: Vec<String> /*InstanceId, deduped, insertion order*/, health_check: HealthCheckConfig, tags: BTreeMap<String,String> }`
- `ListenerEntry { protocol: String, load_balancer_port: i32, instance_protocol: String /*defaults to protocol*/, instance_port: i32, ssl_certificate_id: Option<String>, policy_names: Vec<String> }` — keyed logically by `load_balancer_port`.
- `HealthCheckConfig { target: String, interval: i32, timeout: i32, unhealthy_threshold: i32, healthy_threshold: i32 }` — seeded with ELB defaults at create (`Target: "TCP:<instance_port>"`, `Interval: 30`, `Timeout: 5`, `UnhealthyThreshold: 2`, `HealthyThreshold: 10`) so DescribeLoadBalancers can report a real `HealthCheck`.
- Optional per-instance override for health: `instance_states: BTreeMap<String /*InstanceId*/, (String /*State*/, String /*ReasonCode*/, String /*Description*/)>`, defaulting to `InService`/`N/A` for registered instances.

The other 23 operations (tags, policies, listener mutation, subnets/AZs, attributes, account limits, policy types) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/OPERATIONS.md` lists all 29 operations with the input/output members and errors cited above (CreateLoadBalancer -> `DNSName` + `DuplicateAccessPointNameException`; RegisterInstancesWithLoadBalancer/DeregisterInstancesFromLoadBalancer -> `Instances` + `AccessPointNotFoundException`; DescribeLoadBalancers -> `@paginated(inputToken=Marker, outputToken=NextMarker, items=LoadBalancerDescriptions)` with `PageSize` range 1..400; DescribeInstanceHealth -> `InstanceStates`; DeleteLoadBalancer -> no members, no errors). Shape members for `LoadBalancerDescription`, `ListenerDescription`, `Listener` and `InstanceState` (including the documented `State`/`ReasonCode` valid values and the `Instance is not currently registered with the LoadBalancer.` description) come from the Smithy model at `smithy-model/com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7.json`. A grep for `^//! SCAFFOLDED, NOT IMPLEMENTED:` under `src/ops/` matches all 29 handler files, including `create_load_balancer.rs`, `register_instances_with_load_balancer.rs`, `describe_load_balancers.rs`, `describe_instance_health.rs`, `deregister_instances_from_load_balancer.rs` and `delete_load_balancer.rs` — every one returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and stores nothing. No tests cover any behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS client at it: `aws --endpoint-url http://localhost:<port> elb create-load-balancer --load-balancer-name my-elb --listeners Protocol=HTTP,LoadBalancerPort=80,InstanceProtocol=HTTP,InstancePort=8080 --availability-zones us-east-1a` — expect a response containing a `DNSName`; today: HTTP 501 NotImplemented.
2. Repeat the same create-load-balancer call — expect HTTP 400 `DuplicateAccessPointNameException`.
3. `aws elb register-instances-with-load-balancer --load-balancer-name my-elb --instances i-1111111111111111 i-2222222222222222` — expect `Instances` listing both IDs read back from state.
4. `aws elb describe-load-balancers --load-balancer-names my-elb` — expect one `LoadBalancerDescription` whose `DNSName` equals the one returned by create, whose `ListenerDescriptions[0].Listener` matches the HTTP 80 -> 8080 listener created, whose `Instances` contains both registered IDs, and which carries `HealthCheck`, `Scheme` and `CreatedTime`.
5. `aws elb describe-instance-health --load-balancer-name my-elb` — expect two `InstanceStates` entries for the registered IDs; then query an unregistered ID and expect `OutOfService` with ReasonCode `ELB`.
6. `aws elb deregister-instances-from-load-balancer --load-balancer-name my-elb --instances i-1111111111111111` — expect only `i-2222222222222222` remaining, and confirm via describe-load-balancers / describe-instance-health.
7. `aws elb describe-load-balancers --load-balancer-names no-such-elb` — expect HTTP 400 `AccessPointNotFoundException`.
8. `aws elb delete-load-balancer --load-balancer-name my-elb` — expect success; run it a second time and expect success again (idempotent); then `describe-load-balancers --load-balancer-names my-elb` must return `AccessPointNotFoundException`.
