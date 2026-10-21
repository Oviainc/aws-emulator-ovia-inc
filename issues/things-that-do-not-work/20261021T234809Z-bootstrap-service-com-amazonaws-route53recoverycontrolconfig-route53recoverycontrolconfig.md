Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9e4273988d175f89e8a017aa0737ee7107d62a89

# Bootstrap the Route53 Recovery Control Config emulator

_Filed: 2026-10-21T23:48:09Z_
_Operation: service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig | Class: missing_

> _Note: this issue was identified against the code base at commit `9e4273988d175f89e8a017aa0737ee7107d62a89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST stateful slice: the cluster -> control panel -> routing control lifecycle, which is the service's primary resource hierarchy (restJson1, sigv4 name `route53-recovery-control-config`).

### Happy path to implement (in order)

1. **CreateCluster** — `POST /cluster`. Input: `ClusterName` (required, 1..64, `^\S+$`), `ClientToken` (@idempotencyToken), `Tags`, `NetworkType` (`IPV4`|`DUALSTACK`). Must mint a real ARN (`arn:aws:route53-recovery-control::<account>:cluster/<uuid>`), persist the cluster, and return `Cluster{ClusterArn, Name, Status, Owner, NetworkType, ClusterEndpoints}` where `ClusterEndpoints` is the modeled list of `ClusterEndpoint{Endpoint, Region}` (real AWS returns five Regional endpoints). `Status` should be `PENDING` on create and reach `DEPLOYED` on a subsequent read. Repeating the same `ClientToken` with the same body must return the already-created cluster, not a second one.
2. **DescribeCluster** — `GET /cluster/{ClusterArn}`. Must look the stored cluster up by ARN and return exactly what CreateCluster persisted (same name, endpoints, NetworkType, owner); unknown ARN -> `ResourceNotFoundException` (404).
3. **CreateControlPanel** — `POST /controlpanel`. Input: `ClusterArn` (required), `ControlPanelName` (required), `ClientToken`, `Tags`. Must validate that `ClusterArn` refers to a persisted cluster (else 404), mint a control-panel ARN, persist it under that cluster, and return `ControlPanel{ControlPanelArn, ClusterArn, Name, Status, DefaultControlPanel=false, RoutingControlCount=0, Owner}`. `RoutingControlCount` must be computed from stored routing controls, never hardcoded.
4. **CreateRoutingControl** — `POST /routingcontrol`. Input: `ClusterArn` (required), `ControlPanelArn` (optional — when omitted, attach to the cluster's default control panel), `RoutingControlName` (required), `ClientToken`. Must validate both ARNs exist (404 otherwise), mint a routing-control ARN, persist it under the control panel, and return `RoutingControl{RoutingControlArn, ControlPanelArn, Name, Status, Owner}`. The owning control panel's `RoutingControlCount` must now be 1.
5. **ListRoutingControls** — `GET /controlpanel/{ControlPanelArn}/routingcontrols`. Must return the routing controls actually stored under that control panel (not an empty or canned list), honoring `MaxResults` (1..1000) and round-tripping `NextToken` so paging through the stored set yields each item exactly once; unknown control panel -> 404.
6. **DeleteRoutingControl** — `DELETE /routingcontrol/{RoutingControlArn}`. Must remove the routing control from state so a following `DescribeRoutingControl`/`ListRoutingControls` no longer sees it and the control panel's `RoutingControlCount` drops back to 0; unknown ARN -> `ResourceNotFoundException` (404).

Also wire the two teardown operations needed to close the loop without leaking state: **DeleteControlPanel** (`DELETE /controlpanel/{ControlPanelArn}`) and **DeleteCluster** (`DELETE /cluster/{ClusterArn}`), each returning an empty 200 body, 404 on unknown ARN, and `ConflictException` (409) when the resource still has children (control panel with routing controls, cluster with non-default control panels) — that is the documented AWS behavior and it is what makes the delete observable rather than cosmetic.

### Hard requirements

- NEVER STUBS: every one of these handlers must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)`. No canned ARNs, no echoing the request back without storing it, no random ID that is never written. A create followed by a describe/list in a later request must return what was created.
- ARNs minted on create are the only identity clients have; they must be stable, unique, and accepted by the `@httpLabel` reads/deletes above (model pattern `^[A-Za-z0-9:\/_-]*$`, len 1..256).
- Required-member and length/pattern violations (e.g. empty `ClusterName`, `MaxResults` outside 1..1000) must return `ValidationException` (400), not 200.
- Duplicate name within the same parent, or re-create while a resource is `PENDING_DELETION`, returns `ConflictException` (409).
- Done = an integration script running steps 1-6 plus teardown passes with correct bodies at every step, not merely 2xx.

## State Model

In `ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState` (src/state.rs):

- `clusters: HashMap<String /*cluster_arn*/, Cluster>` where `Cluster { arn: String, name: String, status: Status /*PENDING|DEPLOYED|PENDING_DELETION*/, owner: String /*12-digit account*/, network_type: NetworkType, endpoints: Vec<ClusterEndpoint{endpoint: String, region: String}>, tags: HashMap<String,String>, default_control_panel_arn: String }`
- `control_panels: HashMap<String /*control_panel_arn*/, ControlPanel>` where `ControlPanel { arn: String, cluster_arn: String, name: String, status: Status, owner: String, default_control_panel: bool, tags: HashMap<String,String> }` — `RoutingControlCount` is DERIVED by counting `routing_controls` with this `control_panel_arn`, never stored stale.
- `routing_controls: HashMap<String /*routing_control_arn*/, RoutingControl>` where `RoutingControl { arn: String, control_panel_arn: String, cluster_arn: String, name: String, status: Status, owner: String }`
- `idempotency: HashMap<String /*ClientToken*/, String /*created resource ARN*/>` so a replayed `ClientToken` returns the first result.
- `list_tokens: HashMap<String /*NextToken*/, usize /*offset into the stable ordering*/>` (or an encoded offset token) shared by the List* operations.

Everything else (CreateSafetyRule/DescribeSafetyRule/UpdateSafetyRule/DeleteSafetyRule, ListSafetyRules, ListClusters, ListControlPanels, Update{Cluster,ControlPanel,RoutingControl}, Tag/Untag/ListTagsForResource, GetResourcePolicy, ListAssociatedRoute53HealthChecks) stays at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/OPERATIONS.md` lists all 25 bound operations with routes, input/output members and errors: CreateCluster `POST /cluster` -> `Cluster`; DescribeCluster `GET /cluster/{ClusterArn}`; CreateControlPanel `POST /controlpanel` (required `ClusterArn`, `ControlPanelName`) -> `ControlPanel`; CreateRoutingControl `POST /routingcontrol` (required `ClusterArn`, `RoutingControlName`) -> `RoutingControl`; ListRoutingControls `GET /controlpanel/{ControlPanelArn}/routingcontrols` with `@paginated(inputToken=NextToken, outputToken=NextToken, items=RoutingControls)` and `MaxResults` range 1..1000; DeleteRoutingControl/DeleteControlPanel/DeleteCluster with `ResourceNotFoundException (404)`, `ConflictException (409)`, `ValidationException (400)`. Output member shapes confirmed via smithy_query: `Cluster{ClusterArn, ClusterEndpoints, Name, Status, Owner, NetworkType}`, `ControlPanel{ClusterArn, ControlPanelArn, DefaultControlPanel, Name, RoutingControlCount, Status, Owner}`, `RoutingControl{ControlPanelArn, Name, RoutingControlArn, Status, Owner}`, `ClusterEndpoint{Endpoint, Region}`. All 26 files under `src/ops/` (25 ops + mod.rs) match `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:'`, i.e. every handler still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and no test asserts any stateful behavior (only the generated `tests/state_roundtrip.rs` state-API example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `route53-recovery-control-config`.
2. `CreateCluster{ClusterName="demo-cluster", NetworkType="IPV4"}` -> expect 200 with a minted `Cluster.ClusterArn`, `Name=demo-cluster`, non-empty `ClusterEndpoints`; today: HTTP 501 NotImplemented.
3. `DescribeCluster{ClusterArn=<arn from step 2>}` -> expect the same cluster (name, endpoints, NetworkType) read back from state.
4. `CreateControlPanel{ClusterArn=<cluster arn>, ControlPanelName="demo-panel"}` -> expect 200 with a minted `ControlPanelArn`, `ClusterArn` echoed from state, `RoutingControlCount=0`.
5. `CreateRoutingControl{ClusterArn=<cluster arn>, ControlPanelArn=<panel arn>, RoutingControlName="demo-rc"}` -> expect 200 with a minted `RoutingControlArn`.
6. `ListRoutingControls{ControlPanelArn=<panel arn>}` -> expect exactly one entry whose `RoutingControlArn`/`Name` match step 5; `DescribeControlPanel` (or the next create) must now report `RoutingControlCount=1`.
7. `ListRoutingControls{ControlPanelArn=<panel arn>, MaxResults=1}` after creating a second routing control -> expect one item plus a `NextToken`, and paging with that token returns the second item and no repeats.
8. `DeleteRoutingControl{RoutingControlArn=<rc arn>}` -> expect 200, then `DescribeRoutingControl` on that ARN -> `ResourceNotFoundException` 404 and `ListRoutingControls` no longer lists it.
9. `DeleteCluster{ClusterArn=<cluster arn>}` while the control panel still exists -> expect `ConflictException` 409; after `DeleteControlPanel`, `DeleteCluster` -> 200 and `DescribeCluster` -> 404.
10. Negative: `CreateControlPanel{ClusterArn="arn:aws:route53-recovery-control::111122223333:cluster/does-not-exist", ControlPanelName="x"}` -> expect `ResourceNotFoundException` 404; `CreateCluster{ClusterName=""}` -> expect `ValidationException` 400.
