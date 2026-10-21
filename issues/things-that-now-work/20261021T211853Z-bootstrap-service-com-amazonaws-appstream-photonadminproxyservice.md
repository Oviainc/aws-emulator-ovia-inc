Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the appstream-photonadminproxyservice emulator

_Filed: 2026-10-21T21:18:53Z_
_Operation: service-com-amazonaws-appstream-photonadminproxyservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 89 operations in `services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented`, and the service holds no state. No AppStream client workflow can run at all.

This issue scopes the FIRST, SMALL slice: the stack/fleet lifecycle, which is AppStream's primary resource pair. Implement these 6 operations statefully (awsJson1_1, `POST /` with `X-Amz-Target: PhotonAdminProxyService.<OperationName>`, per README/OPERATIONS.md):

1. **CreateStack** — input `Name` (required, pattern `^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`), `Description`, `DisplayName`, `StorageConnectors`, `RedirectURL`, `FeedbackURL`, `UserSettings`, `ApplicationSettings`, `AccessEndpoints`, `EmbedHostDomains`, `Tags`. Must persist a new Stack and return output member `Stack` built FROM the stored record: `Arn` (`arn:aws:appstream:<region>:<account>:stack/<Name>`), `Name`, `Description`, `DisplayName`, `CreatedTime` (now), the echoed-but-STORED config members, and empty `StackErrors`. A second CreateStack with the same `Name` must fail `ResourceAlreadyExistsException` (HTTP 400).

2. **CreateFleet** — input `Name` (required), `InstanceType` (required), `ImageName`/`ImageArn`, `FleetType` (ALWAYS_ON|ELASTIC|ON_DEMAND, default ON_DEMAND), `ComputeCapacity`, `VpcConfig`, `Description`, `DisplayName`, `MaxUserDurationInSeconds`, `DisconnectTimeoutInSeconds`, `IdleDisconnectTimeoutInSeconds`, `MaxConcurrentSessions`, `StreamView`, `Platform`, `Tags`. Must persist the fleet and return output `Fleet` with the model's required members populated from state: `Arn`, `Name`, `InstanceType`, `State` (`STOPPED` for a freshly created fleet), and `ComputeCapacityStatus` with required `Desired` derived from the requested `ComputeCapacity` (`Running`/`InUse`/`Available` = 0 while stopped), plus `CreatedTime` and empty `FleetErrors`. Duplicate `Name` → `ResourceAlreadyExistsException`.

3. **AssociateFleet** — input `FleetName`, `StackName` (both required), empty output. Must look BOTH names up in state and return `ResourceNotFoundException` (HTTP 404) if either is absent; on success record the association in both directions so it is observable later.

4. **DescribeStacks** — input `Names` (optional list), `NextToken`; output `Stacks` + `NextToken`. With no `Names`, return every stored stack; with `Names`, return only those and raise `ResourceNotFoundException` for a name that was never created. The returned `Stack` structures must be the persisted ones (same `Arn`, `CreatedTime`, `Description`, `UserSettings`, …), not re-synthesized defaults.

5. **DescribeFleets** — input `Names`, `NextToken`; output `Fleets` + `NextToken`. Same filtering/not-found rules; each `Fleet` must reflect stored state including current `State` and `ComputeCapacityStatus`.

6. **DeleteFleet / DeleteStack** — input `Name` (required), empty output. Must remove the record so the matching Describe no longer returns it (`ResourceNotFoundException` when asked by name), and must return `ResourceNotFoundException` (404) for an unknown name. A fleet still associated with a stack must fail `ResourceInUseException` (HTTP 400) until `DisassociateFleet`/the stack is removed — the association recorded in step 3 is what makes this check real.

NEVER STUBS: each of these must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (never a fresh engine). Hardcoded `Fleet`/`Stack` bodies, echoing the input without storing it, or minting an ARN without a record are failures of this issue.

## State Model

In `ComAmazonawsAppstreamPhotonadminproxyserviceState`:

- `stacks: BTreeMap<String /*Name*/, StackRecord>` where `StackRecord { arn: String, name: String, description: Option<String>, display_name: Option<String>, created_time: SystemTime, storage_connectors: Vec<StorageConnector>, redirect_url: Option<String>, feedback_url: Option<String>, user_settings: Vec<UserSetting>, application_settings: Option<ApplicationSettings>, access_endpoints: Vec<AccessEndpoint>, embed_host_domains: Vec<String>, streaming_experience_settings: Option<..>, tags: BTreeMap<String,String>, stack_errors: Vec<StackError> }`
- `fleets: BTreeMap<String /*Name*/, FleetRecord>` where `FleetRecord { arn: String, name: String, instance_type: String, fleet_type: FleetType, image_name: Option<String>, image_arn: Option<String>, state: FleetState /* STOPPED on create */, desired_capacity: i32, running: i32, in_use: i32, available: i32, vpc_config: Option<VpcConfig>, description: Option<String>, display_name: Option<String>, max_user_duration_in_seconds: Option<i32>, disconnect_timeout_in_seconds: Option<i32>, idle_disconnect_timeout_in_seconds: Option<i32>, max_concurrent_sessions: Option<i32>, stream_view: Option<StreamView>, platform: Option<PlatformType>, created_time: SystemTime, tags: BTreeMap<String,String>, fleet_errors: Vec<FleetError> }`
- `fleet_stack_associations: BTreeSet<(String /*FleetName*/, String /*StackName*/)>` — written by AssociateFleet, read by DeleteFleet/DeleteStack for the `ResourceInUseException` check and later by ListAssociatedFleets/ListAssociatedStacks.
- A shared `account_id`/`region` source for ARN construction so ARNs are stable across calls.

All three structures live in the single shared engine store behind the state lock, so a create in one request is visible to the next request.

Everything else in the service (images, image builders, app blocks, applications, entitlements, users, sessions, streaming URLs, tagging, StartFleet/StopFleet, …) stays 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-appstream-photonadminproxyservice/OPERATIONS.md` (89 operations, protocol awsJson1_1, target prefix `PhotonAdminProxyService.`) documents the exact contracts cited: CreateStack (required `Name`; output `Stack`; errors include `ResourceAlreadyExistsException`), CreateFleet (required `Name`+`InstanceType`; output `Fleet`), AssociateFleet (required `FleetName`+`StackName`; no output members; `ResourceNotFoundException` 404), DescribeFleets/DescribeStacks (`Names`/`NextToken` in, `Fleets`/`Stacks`+`NextToken` out, `ResourceNotFoundException` 404), DeleteFleet/DeleteStack (required `Name`; `ResourceInUseException` 400, `ResourceNotFoundException` 404). `smithy_query shape=Fleet,Stack,ComputeCapacityStatus` confirms the @required response members a Describe must fill: Fleet.{Arn,Name,InstanceType,State,ComputeCapacityStatus}, ComputeCapacityStatus.Desired, Stack.Name. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every handler file in the crate, including create_stack.rs, create_fleet.rs, associate_fleet.rs, describe_fleets.rs, describe_stacks.rs, delete_fleet.rs and delete_stack.rs — so all of the above answer 501 today and no test asserts any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AppStream client at it (endpoint prefix `appstream2`, signing name `appstream`).
2. `CreateStack(Name="demo-stack", Description="demo", DisplayName="Demo")` → expect 200 with `Stack.Arn` ending `stack/demo-stack`, `Stack.Name="demo-stack"`, a `CreatedTime`; actual: HTTP 501 NotImplemented.
3. `CreateFleet(Name="demo-fleet", InstanceType="stream.standard.small", ImageName="base-image", ComputeCapacity={DesiredInstances:1}, FleetType="ON_DEMAND")` → expect 200 with `Fleet.State="STOPPED"` and `Fleet.ComputeCapacityStatus.Desired=1`.
4. `AssociateFleet(FleetName="demo-fleet", StackName="demo-stack")` → expect 200; then `AssociateFleet(FleetName="nope", StackName="demo-stack")` → expect ResourceNotFoundException (404).
5. `DescribeStacks()` and `DescribeFleets()` → expect exactly the created stack and fleet with the same ARNs/CreatedTime/Description as returned by the creates; `DescribeFleets(Names=["missing"])` → ResourceNotFoundException.
6. `DeleteFleet(Name="demo-fleet")` while still associated → expect ResourceInUseException (400); `DisassociateFleet` then `DeleteFleet`, then `DeleteStack(Name="demo-stack")` → expect 200 each.
7. `DescribeFleets(Names=["demo-fleet"])` and `DescribeStacks(Names=["demo-stack"])` → expect ResourceNotFoundException, proving the deletes mutated persisted state.
