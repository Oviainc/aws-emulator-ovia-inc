Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs.cluster-create-active-delete-state-transition-lifecycle pcs.create-cluster-duplicate-name-conflict-rejected pcs.create-cluster-invalid-scheduler-version-rejected pcs.update-cluster-invalid-scheduler-version-rejected
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs.cluster-create-active-delete-state-transition-lifecycle|run#13|AssertionError|assert "SLURMCTLD" in endpoint_types, f"ACTIVE cluster missing a SLURMCTLD endpoint: {cluster.get('endpoints')}"
Issue-Key: pcs.create-cluster-duplicate-name-conflict-rejected|run#13|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: pcs.create-cluster-invalid-scheduler-version-rejected|run#5|AssertionError|raise AssertionError(f"CreateCluster with a bad scheduler version SUCCEEDED: {result!r}")
Issue-Key: pcs.update-cluster-invalid-scheduler-version-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: 2a533320acecc5e3d5510f43fe45b8adcca9ff2d

# pcs: 4 net-new root causes — ACTIVE clusters expose no SLURMCTLD endpoint, CreateCluster accepts an unsupported scheduler version and authors its own duplicate-name message, and UpdateCluster is unimplemented

> _Note: this issue was identified against the code base at commit `2a533320acecc5e3d5510f43fe45b8adcca9ff2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Four independent root causes in the PCS emulator (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the behavioral suite simparity-behavioral-aws-v1. ALL FOUR must be closed before this issue is submitted; each cause names its own failing check ids so each can be re-replayed independently.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0).

Related but NOT covering: open issue 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md (not-found message text, ValidationException framing, CreateComputeNodeGroup ordering, tagging + RegisterComputeNodeGroupInstance stubs) and open issue 20261029T190856Z-pcs-5-net-new-root-causes-cluster-queue.md (wire corpus: client-error status class, cluster not-found body, child-op AccessDenied class, node-group validation ordering, CreateCluster subnet validation). Neither of them carries any of the four causes below and neither names any of the four check ids in trace_ids: the cluster endpoints representation, the duplicate-name ConflictException message, the scheduler-version gate on CreateCluster and the unimplemented UpdateCluster handler are all net-new here.

=== Cause 1 of 4: a cluster that reaches ACTIVE exposes no endpoints at all, so the SLURMCTLD control-plane endpoint is never published ===
Failing check: pcs.cluster-create-active-delete-state-transition-lifecycle
AWS: Smithy: Cluster.endpoints -> Endpoints (list of Endpoint), documented "The list of endpoints available for interaction with the scheduler."; Endpoint requires type (EndpointType enum), privateIpAddress and port. Recorded behavior for the text of the values: once a SLURM cluster finishes provisioning and GetCluster reports status ACTIVE, the returned cluster carries an endpoints list containing an entry whose type is SLURMCTLD (the Slurm controller), with its required privateIpAddress and port members populated. The requirement is that an ACTIVE cluster publishes a SLURMCTLD-typed endpoint with the modeled required members present; the particular IP address and port number in any one recording are that cluster's per-run values and are NOT part of the requirement.
Emulator: GetCluster on a cluster the emulator has already moved to ACTIVE returns no endpoints member at all (the check saw cluster.get('endpoints') == None). CreateCluster mints the record with `endpoints: Vec::new()` and nothing on the CREATING -> ACTIVE settle path ever fills it in.
Why: the endpoints list is the ONLY way a client learns where to reach the Slurm controller. Any caller that creates a cluster and then connects to, or configures, the scheduler (or merely asserts the cluster is usable once ACTIVE) gets an empty/absent list under the emulator and cannot proceed, while against PCS it gets a reachable SLURMCTLD endpoint. The modeled required members of Endpoint are never exercised either.
Fix: the cluster activation step in this crate's state module (the Pending/PendingChange::Activate settle path that flips STATUS_CREATING to ACTIVE) must populate the record's endpoints with a SLURMCTLD entry, and the cluster record -> API projection (ClusterRecord::to_api) must serialize it; the record is minted with an empty endpoints vector in ops/create_cluster.rs (line 312).
Traces: pcs.cluster-create-active-delete-state-transition-lifecycle.

=== Cause 2 of 4: the duplicate-cluster-name ConflictException carries an emulator-authored message that echoes the requested name ===
Failing check: pcs.create-cluster-duplicate-name-conflict-rejected
AWS: Smithy: CreateCluster models ConflictException (errorKind client, httpError 409) with required members message, resourceId and resourceType, and its docs list "A cluster with the same name already exists." as an example. The model does not state the message text, so recorded behavior only: a CreateCluster whose clusterName is already taken by an existing cluster is refused with code ConflictException and the FIXED sentence "A cluster with the name you specified already exists." — the whole sentence is fixed text, it names no cluster and embeds no value from the request. The error code already matches, and the effect is correctly absent (exactly one cluster of that name survives, with an unchanged createdAt).
Emulator: builds the message from the request instead — it answered 'a cluster named <the clusterName from the request> already exists' (recorded example: 'a cluster named sim2real-dupname09b3b8879e14 already exists', where the name is that run's value). See ops/create_cluster.rs line 282, `format!("a cluster named {} already exists", input.cluster_name)`.
Why: a client that classifies or surfaces a PCS name conflict by the service's wording never matches, and the emulator leaks the caller-supplied name into a message where PCS does not, so message-equality assertions, log scrapers and user-facing error text all differ from production. The same handler's clientToken-reuse branch (line 266) authors its own sentence the same way and should be reviewed with it.
Fix: the ConflictException construction in the duplicate-name branch of ops/create_cluster.rs (and the sibling clientToken branch) — emit PCS's fixed sentence and keep resourceId/resourceType populated per the modeled required members.
Traces: pcs.create-cluster-duplicate-name-conflict-rejected.

=== Cause 3 of 4: CreateCluster accepts any scheduler version string and creates the cluster, instead of refusing an unsupported Slurm version ===
Failing check: pcs.create-cluster-invalid-scheduler-version-rejected
AWS: Smithy: CreateClusterRequest.scheduler -> SchedulerRequest (required) with required members type (SchedulerType enum) and version (String), whose docs state "Valid Values: 24.11 | 25.05 | 25.11 | 26.05"; CreateCluster models ValidationException (errorKind client, httpError 400), whose own docs list "The scheduler version isn't supported." as an example. version is an unconstrained String in the model, so the supported-set check is a service-side rule, not a boundary constraint. Recorded behavior for the text: a version outside the supported set is refused with HTTP 400, code ValidationException and the message "Scheduler version not found: type=<scheduler type from the request>, version=<version from the request>" — fixed text "Scheduler version not found: type=" and ", version=", with the type and version values taken verbatim from the request (recorded example: type=SLURM, version=1.0-not-a-real-version). No cluster is created: ListClusters afterwards contains nothing under that name.
Emulator: answered HTTP 200 with a created cluster in CREATING whose scheduler.version is the bogus string ('1.0-not-a-real-version'), i.e. the refusal is missing entirely. ops/create_cluster.rs validates only clusterName's pattern and the sizes of the caller-sized stored lists before minting the id and inserting the record; there is no scheduler-version gate anywhere on the path.
Why: the guardrail is absent, which is worse than wrong wording — a client relying on CreateCluster to reject an unsupported scheduler gets an id and an ACTIVE-bound cluster that could never have been provisioned, so the failure surfaces later or never, infrastructure tests asserting the guardrail pass against the emulator and fail against PCS, and the bogus cluster pollutes state and counts against the cluster quota.
Fix: add the supported-scheduler-version check to the CreateCluster handler (ops/create_cluster.rs, before the id is minted and the record inserted), returning the modeled ValidationException with PCS's "Scheduler version not found: type=<type>, version=<version>" message rendered from the request and persisting nothing. UpdateCluster needs the same supported-set knowledge (see Cause 4), so the supported-version set belongs in the crate's state/validation module, shared by both handlers.
Traces: pcs.create-cluster-invalid-scheduler-version-rejected.

=== Cause 4 of 4: UpdateCluster is an unimplemented scaffold and answers 501 NotImplemented for every request ===
Failing check: pcs.update-cluster-invalid-scheduler-version-rejected (this check is NOT marked ALREADY FILED or OWNED in the replay report, and no open issue names UpdateCluster)
AWS: Smithy: UpdateCluster (idempotent) takes clusterIdentifier (required, @pattern ^(pcs_[a-zA-Z0-9]+|[A-Za-z][A-Za-z0-9-]{2,40})$), optional scheduler -> UpdateSchedulerRequest (required member version, docs "Valid Values: 24.05 | 24.11 | 25.05 | 25.11 | 26.05") and optional slurmConfiguration, returns the Cluster, and models AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429) and ValidationException (400). Two behaviors are exercised here. (a) Refusal: an UpdateCluster naming an unsupported scheduler version is refused with HTTP 400, code ValidationException and the fixed sentence "AWS PCS doesn't support the scheduler version you provided." (all fixed text, naming no version), and has no effect — the cluster stays ACTIVE on its original scheduler.version. Note this sentence differs from CreateCluster's version message in Cause 3; each path has its own wording. (b) Success: an UpdateCluster that sets slurmConfiguration.scaleDownIdleTimeInSeconds returns the cluster with status UPDATING or ACTIVE and the new value persists, readable through GetCluster once the cluster settles back to ACTIVE.
Emulator: every UpdateCluster answers error code NotImplemented (HTTP 501). services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/update_cluster.rs is still the generated scaffold: line 4 carries '//! SCAFFOLDED, NOT IMPLEMENTED' and line 193 returns crate::scaffolded::Scaffolded::not_implemented("UpdateCluster").
Why: the whole update surface of a cluster is unusable — a client can neither change the scheduler version nor tune Slurm settings, and a 501 is neither modeled nor retry-classified by the SDK, so error handling written for ValidationException/ConflictException/ResourceNotFoundException never triggers and a caller sees an unmodeled server failure for a well-formed request.
Fix: implement ops/update_cluster.rs against the cluster store in the crate's state module — resolve clusterIdentifier (id or name), refuse an unsupported scheduler.version with the modeled ValidationException and PCS's fixed sentence while leaving the record untouched, apply slurmConfiguration changes to the stored record and report UPDATING/ACTIVE through the existing pending-settle machinery, then delete the SCAFFOLDED marker line at the top of the file.
Traces: pcs.update-cluster-invalid-scheduler-version-rejected (and the positive control at the tail of pcs.create-cluster-duplicate-name-conflict-rejected, which also calls UpdateCluster).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: pcs 0/13 passed (0%), 11 mismatched, 2 errored. Of the 13 failures, 1 is ALREADY FILED (blocked by the unimplemented TagResource stub), 8 are OWNED by open issues, and the 4 below are net-new and clustered here.

Cause 1 — pcs.cluster-create-active-delete-state-transition-lifecycle [mismatch]:
  AssertionError: ACTIVE cluster missing a SLURMCTLD endpoint: None
  (the cluster did reach status ACTIVE; GetCluster returned no endpoints member, so endpoint_types was empty. Expected an endpoints entry with type SLURMCTLD plus its required privateIpAddress and port.)

Cause 2 — pcs.create-cluster-duplicate-name-conflict-rejected [mismatch]:
  AssertionError: expected message 'A cluster with the name you specified already exists.', got 'a cluster named sim2real-dupname09b3b8879e14 already exists'
  (code ConflictException matched; the cluster name in the observed message is that run's generated name, the expected sentence is wholly fixed text. Confirmed in the checkout: ops/create_cluster.rs line 282 formats the message from input.cluster_name.)

Cause 3 — pcs.create-cluster-invalid-scheduler-version-rejected [mismatch]:
  AssertionError: CreateCluster with a bad scheduler version SUCCEEDED: {'cluster': {'name': 'sim2real-badver...', 'id': 'pcs_e220a8397b1dcdaf', 'status': 'CREATING', 'scheduler': {'type': 'SLURM', 'version': '1.0-not-a-real-version'}, ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected HTTP 400 / ValidationException with message 'Scheduler version not found: type=SLURM, version=1.0-not-a-real-version' and no cluster created; observed HTTP 200 and a persisted cluster. (The id, name and timestamps in the observed body are per-run values.) Confirmed in the checkout: ops/create_cluster.rs performs only the clusterName pattern check (lines 198-205) and stored-list size checks (lines 212-251) before minting and inserting the record (lines 297-325); no scheduler-version check exists.

Cause 4 — pcs.update-cluster-invalid-scheduler-version-rejected [mismatch]:
  AssertionError: expected ValidationException, got NotImplemented
  Expected HTTP 400 / ValidationException with message "AWS PCS doesn't support the scheduler version you provided." and the cluster left ACTIVE on version 25.05; observed HTTP 501 / NotImplemented. Confirmed in the checkout: ops/update_cluster.rs line 4 '//! SCAFFOLDED, NOT IMPLEMENTED' and line 193 'Err(crate::scaffolded::Scaffolded::not_implemented("UpdateCluster"))'.

Per-service bootstrap gate: pcs is bootstrapped (service-com-amazonaws-pcs-awsparallelcomputingservice), the service answers, so these are fidelity findings, not bootstrap gaps.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK PCS client at it (awsJson1_0, sigv4). Replay the four check ids in trace_ids, or reproduce by hand:

Cause 1: c = pcs.create_cluster(clusterName=<valid name>, scheduler={'type':'SLURM','version':'25.05'}, size='SMALL', networking={'subnetIds':[<an existing subnet>], 'securityGroupIds':[<the default sg>]}); poll pcs.get_cluster(clusterIdentifier=c['cluster']['id']) until status == 'ACTIVE', then read cluster['endpoints']. Expect a list containing an entry with type 'SLURMCTLD' and populated privateIpAddress and port; observe the member missing entirely (None).

Cause 2: create a cluster as above and wait for ACTIVE, then call pcs.create_cluster with the SAME clusterName. Expect ConflictException with Message exactly 'A cluster with the name you specified already exists.'; observe 'a cluster named <that name> already exists'. (ListClusters correctly still shows exactly one cluster of that name with an unchanged createdAt.)

Cause 3: pcs.create_cluster(clusterName=<valid unused name>, scheduler={'type':'SLURM','version':'1.0-not-a-real-version'}, size='SMALL', networking={'subnetIds':[<an existing subnet>]}). Expect HTTP 400 ValidationException, Message 'Scheduler version not found: type=SLURM, version=1.0-not-a-real-version', and pcs.list_clusters() showing no cluster under that name; observe HTTP 200, a cluster in CREATING carrying the bogus version, and that cluster present in ListClusters.

Cause 4: create a cluster on scheduler version 25.05 and wait for ACTIVE, then (a) pcs.update_cluster(clusterIdentifier=<its id>, scheduler={'version':'1.0-not-a-real-version'}) — expect HTTP 400 ValidationException with Message "AWS PCS doesn't support the scheduler version you provided." and GetCluster still reporting version 25.05 and status ACTIVE; and (b) pcs.update_cluster(clusterIdentifier=<its id>, slurmConfiguration={'scaleDownIdleTimeInSeconds':700}) — expect status UPDATING or ACTIVE and the value to persist through GetCluster. Observe HTTP 501 NotImplemented for both.
