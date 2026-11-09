Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftserverless-data-api-batch-sql-round-trip
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftserverless-data-api-batch-sql-round-trip
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.endpoint.address
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.endpoint.vpcEndpoints[*]
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.patchVersion
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.pricePerformanceTarget
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.trackName
Issue-Key: redshift-serverless|GetWorkgroup|2xx->2xx|$.workgroup.workgroupVersion
Issue-Commit: 125deed380da51cfd31ee17449b40c32c4b4107b

# redshift-serverless Workgroup projection omits server-derived release, track, price-performance and VPC-endpoint fields

> _Note: this issue was identified against the code base at commit `125deed380da51cfd31ee17449b40c32c4b4107b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkgroup

## Description
Replay of the wire corpus simparity-wire-aws-v1 against the emulator (65 steps, 50 passed, 4 divergence clusters) found that GetWorkgroup answers the CORRECT HTTP status (200) with the correct Workgroup identity members, but the `workgroup` object it projects is missing or wrong in every member Redshift Serverless DERIVES server-side rather than echoing from the create request. The status, the operation routing and the request-supplied members are all right, so nothing here is a transport, auth or routing problem: the single root cause is that the Workgroup record is materialised at CreateWorkgroup time from the request plus a build constant, with no derivation of the service-owned fields, and GetWorkgroup/ListWorkgroups then faithfully report that under-populated record back.

This bundle carries FIVE causes (Cause 1 of 5 .. Cause 5 of 5); all five must be closed before submitting. All five are in the same component and are all observed on the single fixture step named in Issue-Trace/Issue-Step. No open issue covers them: the open redshift-serverless issues are operation stubs (20261029T191808Z-redshift-serverless-updateusagelimit-stub.md, 20261102T103912Z-redshift-serverless-listtagsforresource-stub.md) or CloudFormation resource-type registration (20261109T124343Z-cloudformation-redshift-serverless-aws-redshiftserverless-namespace-is-not.md, 20261109T124343Z-cloudformation-ec2-redshift-serverless-createstack-refuses-aws-redshiftserverless.md), none of which touch the Workgroup response projection. The replay also reports $.workgroup.endpoint.address under a `host_not_own` verdict with expected and actual IDENTICAL (the emulator serves exactly the recorded AWS-shaped DNS name); a hostname is a per-environment value, so that line is deliberately NOT filed as a cause here.

=== Cause 1 of 5: workgroupVersion reports a two-part build constant instead of the serving Redshift Serverless release version ===
AWS: Smithy (services/service-com-amazonaws-redshiftserverless-redshiftserverless/smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json): `Workgroup$workgroupVersion` (String) — 'The Amazon Redshift Serverless version of your workgroup.' Recorded behavior: the member carries a full three-part Redshift Serverless release version of the form `<major>.<minor>.<build>` (the recording's run shows `1.0.535482`); the build segment is the release the workgroup is actually running.
Emulator: the member is the literal `1.0` — a two-part constant baked into the build (`pub const WORKGROUP_VERSION: &str = "1.0"`, src/state.rs:68-71, whose own doc says 'One emulator build is one release, so the value is a property of the build and not of any request'), written into the record at src/ops/create_workgroup.rs:280.
Why: `workgroupVersion` is how a client decides whether a workgroup is running a release that supports a feature, and release-gating code parses it as `major.minor.build`. A value with no build segment either fails that parse or compares as older than every real release, so feature detection and 'is this workgroup up to date' logic silently take the wrong branch. The requirement is the SHAPE and provenance — the full release version of the release the emulator serves — not the recording's literal `1.0.535482`, which is one run's release.
Fix: the redshift-serverless workgroup materialisation in the CreateWorkgroup handler (services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_workgroup.rs) together with the release constant it reads in src/state.rs: report a full three-part release version for the release this emulator serves, so every reader of the stored record (GetWorkgroup, ListWorkgroups, UpdateWorkgroup) reports it.
Traces: redshiftserverless-data-api-batch-sql-round-trip#task#1 ($.workgroup.workgroupVersion: value_mismatch).

=== Cause 2 of 5: patchVersion is absent from the Workgroup entirely ===
AWS: Smithy: `Workgroup$patchVersion` (String) — 'The patch version of your Amazon Redshift Serverless workgroup.' Recorded behavior: every workgroup reports `patchVersion`, a numeric patch level string for the release named in `workgroupVersion` (the recording's run shows `206`).
Emulator: the member is MISSING from the response. Nothing in the materialisation at src/ops/create_workgroup.rs:253-292 sets it, so it falls to `..Workgroup::default()` (None) and the serializer omits it; no later write populates it either, so GetWorkgroup can never report it.
Why: `patchVersion` is the second half of the version pair clients gate on (release + patch). A consumer that reads `workgroup['patchVersion']` gets a KeyError/absent field on every workgroup, and any patch-level comparison is impossible. The recording's `206` is one release's patch level, not the requirement; the requirement is that the member is reported for the release the emulator serves.
Fix: the same CreateWorkgroup workgroup materialisation in services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_workgroup.rs (with the patch level alongside the release constant in src/state.rs) must populate `patchVersion`.
Traces: redshiftserverless-data-api-batch-sql-round-trip#task#1 ($.workgroup.patchVersion: missing).

=== Cause 3 of 5: pricePerformanceTarget is echoed from the request only, so a workgroup created without one reports no target instead of the DISABLED default ===
AWS: Smithy: `Workgroup$pricePerformanceTarget` targets `PerformanceTarget` — 'An object that represents the price performance target settings for the workgroup' — whose `status` member is the `PerformanceTargetStatus` enum with exactly the values `ENABLED` and `DISABLED` ('Whether the price performance target is enabled for the workgroup'), and whose `level` is an Integer @range(min 1, max 100). `CreateWorkgroup`'s `pricePerformanceTarget` input is OPTIONAL. Recorded behavior: a workgroup created WITHOUT a price-performance target still reports `pricePerformanceTarget` with `status` `DISABLED` and no `level` — the feature's off-state is reported explicitly, not by omission.
Emulator: the member is MISSING. The handler assigns `price_performance_target: input.price_performance_target` (src/ops/create_workgroup.rs:284) — a straight echo of the request — so a create that named none stores none and every read omits the member.
Why: a client that reads `workgroup['pricePerformanceTarget']['status']` to show or to toggle AI-driven scaling gets an absent field rather than the explicit `DISABLED`, so it cannot distinguish 'the target is off' from 'this API does not report targets', and code that round-trips the object into UpdateWorkgroup drops the setting.
Fix: the CreateWorkgroup workgroup materialisation in services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_workgroup.rs must default `pricePerformanceTarget` to a `PerformanceTarget` whose `status` is the modelled off value `DISABLED` when the request names none, instead of echoing the optional input.
Traces: redshiftserverless-data-api-batch-sql-round-trip#task#1 ($.workgroup.pricePerformanceTarget: missing).

=== Cause 4 of 5: trackName is echoed from the request only, so a workgroup created without one reports no track instead of the current track it was assigned to ===
AWS: Smithy: `Workgroup$trackName` (TrackName, @length 1..256, @pattern ^[a-zA-Z0-9_]+$) — 'The name of the track for the workgroup'; and `CreateWorkgroup`'s own input documentation states the rule outright: 'An optional parameter for the name of the track for the workgroup. If you don't provide a track name, the workgroup is assigned to the current track.' Recorded behavior: a create that named no track reports the assigned track name (the recording's run shows `current`).
Emulator: the member is MISSING. The handler assigns `track_name: input.track_name` (src/ops/create_workgroup.rs:285), echoing the optional input, so no track is stored and no read reports one.
Why: the documented rule is that a workgroup ALWAYS has a track; `trackName` is what a client reads to know which release train the workgroup follows and what it passes back when pinning or moving a track. An absent member makes a workgroup look untracked, and a client that round-trips the value into UpdateWorkgroup has nothing to send.
Fix: the CreateWorkgroup workgroup materialisation in services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_workgroup.rs must assign the service's current track when the request names none, applying the default the input documentation states, rather than leaving the member unset.
Traces: redshiftserverless-data-api-batch-sql-round-trip#task#1 ($.workgroup.trackName: missing).

=== Cause 5 of 5: endpoint.vpcEndpoints is always an empty list — the workgroup's managed VPC endpoint and its per-subnet network interfaces are never derived ===
AWS: Smithy: `Endpoint$vpcEndpoints` targets `VpcEndpointList` of `VpcEndpoint` — 'An array of VpcEndpoint objects' — where `VpcEndpoint` carries `vpcEndpointId` ('The connection endpoint ID for connecting to Amazon Redshift Serverless'), `vpcId` ('The VPC identifier that the endpoint is associated with') and `networkInterfaces` (`NetworkInterface`: `networkInterfaceId`, `subnetId`, `privateIpAddress`, `availabilityZone`). Recorded behavior: the workgroup's endpoint reports ONE VpcEndpoint, carrying the VPC the workgroup's subnets belong to and one NetworkInterface per subnet the workgroup was created with — each interface naming that subnet, its availability zone and a private IPv4 address inside it (the recording asserts the ids and addresses by SHAPE, '<shaped-not-asserted>', because they are server-minted and per-environment; the subnet ids and AZs are the ones the create supplied).
Emulator: `$.workgroup.endpoint.vpcEndpoints[0]` is absent because the list is hard-coded empty: `vpc_endpoints: Some(Vec::new())` in the derived `Endpoint` at src/ops/create_workgroup.rs:271-278. The endpoint is derived from the name and port only (`crate::state::endpoint_address`, src/state.rs:320-327); the workgroup's `subnetIds` are stored but never turned into interfaces.
Why: `vpcEndpoints` is how a client discovers the private connectivity of a workgroup — which VPC it is reachable in, which subnets and AZs it has interfaces in, and which endpoint id to reference from security-group or DNS configuration. An always-empty list makes a workgroup look as if it has no managed VPC endpoint at all, so connectivity/placement checks (is this workgroup reachable from my subnet? is it multi-AZ?) all read false, and tooling that indexes interfaces by subnet finds nothing.
Fix: the redshift-serverless endpoint derivation shared by the workgroup writers — `endpoint_address` and the `Endpoint` construction it feeds in services/service-com-amazonaws-redshiftserverless-redshiftserverless/src/ops/create_workgroup.rs, with the derivation helper in src/state.rs — must derive one VpcEndpoint for the workgroup (a minted endpoint id and the VPC its subnets belong to) with one NetworkInterface per subnet the workgroup carries, each naming that subnet, its availability zone and a private address, and store it on the record so GetWorkgroup/ListWorkgroups report it.
Traces: redshiftserverless-data-api-batch-sql-round-trip#task#1 ($.workgroup.endpoint.vpcEndpoints[0]: missing).

## Evidence
Replay summary (trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, emulator_detail '/__admin/health answered 200 and the :44803 data listener accepts'): steps_total 65, steps_passed 50, steps_mismatched 15, steps_diverged 4, score_pct 76.9, divergence_count 4. This cluster is verdict=mismatch at the CORRECT HTTP status, expected 200 / actual 200, so the divergence is purely in the response body.

Divergence cluster (divergence_key `service=redshift-serverless|GetWorkgroup|status=200->200|keys=$.workgroup.endpoint.address,$.workgroup.endpoint.vpcEndpoints[0],$.workgroup.patchVersion,$.workgroup.pricePerformanceTarget,$.workgroup.trackName,$.workgroup.workgroupVersion`), fixture redshiftserverless-data-api-batch-sql-round-trip, step task#1, operation GetWorkgroup, affected_count 1, stub_path empty, owned_by empty:
  $.workgroup.workgroupVersion: value_mismatch (expected '1.0.535482', got '1.0')          -> Cause 1
  $.workgroup.patchVersion: missing (expected '206', got None)                              -> Cause 2
  $.workgroup.pricePerformanceTarget: missing (expected {'status': 'DISABLED'}, got None)   -> Cause 3
  $.workgroup.trackName: missing (expected 'current', got None)                             -> Cause 4
  $.workgroup.endpoint.vpcEndpoints[0]: missing (expected one VpcEndpoint carrying vpcId 'vpc-...', vpcEndpointId '<shaped-not-asserted>' and three NetworkInterfaces — one per workgroup subnet, each with availabilityZone us-west-2a/2b/2c, subnetId 'subnet-...', networkInterfaceId '<shaped-not-asserted>' and privateIpAddress '<shaped-not-asserted>'; got None)  -> Cause 5
  $.workgroup.endpoint.address: host_not_own — expected and actual are the IDENTICAL recorded DNS name; a hostname is a per-environment value and this line is NOT filed as a cause.
(The recorded release '1.0.535482', patch '206', track 'current' and the vpc-/subnet-/interface ids are this run's values; the requirements are that a three-part release version and a patch level are reported, that pricePerformanceTarget reports the modelled off status DISABLED when no target was requested, that trackName reports the track the workgroup was assigned to, and that the endpoint reports its managed VpcEndpoint with one NetworkInterface per subnet.)

Emulator source (services/service-com-amazonaws-redshiftserverless-redshiftserverless):
  src/ops/create_workgroup.rs:253-292 — the whole Workgroup materialisation: `workgroup_version: Some(crate::state::WORKGROUP_VERSION.to_string())` (:280), `price_performance_target: input.price_performance_target` (:284), `track_name: input.track_name` (:285), `endpoint: Some(Endpoint { address: ..., port: ..., vpc_endpoints: Some(Vec::new()) })` (:271-278), and `..Workgroup::default()` (:291) which leaves `patch_version` None.
  src/state.rs:68-71 — `pub const WORKGROUP_VERSION: &str = "1.0"` ('a property of the build and not of any request').
  src/state.rs:320-327 — `endpoint_address(workgroup_name, region)` derives only the DNS name; nothing derives VPC endpoints or interfaces from the stored `subnetIds`.
  src/state.rs:419-448 — `WorkgroupRecord.workgroup` is held whole and 'every reader of this workgroup returns' it, which is why GetWorkgroup, ListWorkgroups and UpdateWorkgroup all report the same under-populated projection and why the fix belongs at materialisation.

Model corroboration (services/service-com-amazonaws-redshiftserverless-redshiftserverless/smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json): Workgroup members workgroupVersion (String), patchVersion (String), trackName (TrackName, @length 1..256, @pattern ^[a-zA-Z0-9_]+$), pricePerformanceTarget (PerformanceTarget); PerformanceTarget.status -> PerformanceTargetStatus enum ['ENABLED','DISABLED']; PerformanceTarget.level Integer @range 1..100; Endpoint.vpcEndpoints -> VpcEndpointList of VpcEndpoint {vpcEndpointId, vpcId, networkInterfaces -> NetworkInterface {networkInterfaceId, subnetId, privateIpAddress, availabilityZone, ipv6Address}}; CreateWorkgroup input documents 'If you don't provide a track name, the workgroup is assigned to the current track.'

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (/__admin/health answers 200 and the data listener accepts).
2. create_namespace(namespaceName=<ns>), then create_workgroup(workgroupName=<wg>, namespaceName=<ns>, subnetIds=[<three subnets in three AZs>]) WITHOUT pricePerformanceTarget and WITHOUT trackName — this is what fixture redshiftserverless-data-api-batch-sql-round-trip does before step task#1.
3. Poll get_workgroup(workgroupName=<wg>) until status is AVAILABLE, then inspect the 200 response's `workgroup` object (fixture step task#1).
4. Cause 1: expect `workgroupVersion` to be a full three-part Redshift Serverless release version (major.minor.build); observe the two-part constant '1.0'.
5. Cause 2: expect a `patchVersion` member; observe it absent.
6. Cause 3: expect `pricePerformanceTarget` == {'status': 'DISABLED'} for a workgroup created with no target; observe the member absent.
7. Cause 4: expect `trackName` to name the track the workgroup was assigned to ('current' in the recording); observe the member absent.
8. Cause 5: expect `endpoint.vpcEndpoints` to hold one VpcEndpoint with a vpcEndpointId, the subnets' vpcId and one networkInterfaces entry per supplied subnet (subnetId + availabilityZone + privateIpAddress); observe an empty list.
9. Re-replay the wire fixture step named in Issue-Trace/Issue-Step (redshiftserverless-data-api-batch-sql-round-trip task#1) to confirm all five causes are closed.
