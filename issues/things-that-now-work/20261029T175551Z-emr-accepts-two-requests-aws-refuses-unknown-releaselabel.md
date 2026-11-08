Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.op-run-job-flow-unknown-release-label-fails emr.op-create-studio-rejects-unresolvable-network emr.modify-instance-fleet-terminated-cluster-rejected emr.cluster-reaches-waiting-with-running-instances
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.op-run-job-flow-unknown-release-label-fails|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Key: emr.op-create-studio-rejects-unresolvable-network|_error_code#3|AssertionError|raise AssertionError(
Issue-Key: emr.modify-instance-fleet-terminated-cluster-rejected|run#7|KeyError|spot_capacity = initial_core_fleet["TargetSpotCapacity"]
Issue-Key: emr.cluster-reaches-waiting-with-running-instances|run#9|AssertionError|assert dns, "a WAITING cluster must report MasterPublicDnsName; got an empty/absent value"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# EMR accepts two requests AWS refuses (unknown ReleaseLabel, unresolvable Studio network) and omits two always-present response members (InstanceFleet target capacities, Cluster.MasterPublicDnsName)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
This issue bundles 4 net-new root causes in the EMR (elasticmapreduce) crate: two operations are overly permissive (they return 200 for input real EMR rejects at submit time, so a negative test can never be graded and a client gets a phantom resource), and two read paths omit members AWS always populates (so a correct client crashes or waits forever).

--- Cause 1 of 4: RunJobFlow accepts a nonexistent ReleaseLabel and launches a cluster ---
AWS: Smithy: RunJobFlow.ReleaseLabel is XmlStringMaxLen256 with no enumeration, and the operation models only InternalServerError — so recorded behavior only: real EMR validates the release label at submit time and refuses an unknown one with HTTP 400, code `ValidationException` and the message `The supplied release label is invalid: <ReleaseLabel>.` (fixed text `The supplied release label is invalid: ` + the requested label + `.`), launching NO cluster. An identical request with a real label (emr-6.15.0) is accepted — so the refusal is specific to the label, not to the request shape.
Emulator: RunJobFlow with ReleaseLabel='emr-99.99.0' SUCCEEDS (the check's error extractor reports code None / message None, i.e. no ClientError at all) and the cluster is created.
Why: a client typo'ing or mis-pinning a release label gets a cluster that cannot exist on AWS instead of a 400; every negative test of release-label validation passes vacuously, and cost/lifecycle code sees a resource AWS would never have created.
Fix: add release-label validation to the RunJobFlow handler's input validation in the EMR crate (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs), rejecting with ValidationException/400 before any cluster state is written. It must share the label catalogue used by DescribeReleaseLabel/ListSupportedInstanceTypes so the three agree on which labels exist.
Traces: emr.op-run-job-flow-unknown-release-label-fails.

--- Cause 2 of 4: CreateStudio accepts an unresolvable VPC/subnet/security-group and an unassumable ServiceRole ---
AWS: Smithy: CreateStudio requires Name, AuthMode, VpcId, SubnetIds, ServiceRole, WorkspaceSecurityGroupId, EngineSecurityGroupId, DefaultS3Location and models InvalidRequestException (client, 400) alongside InternalServerException. Recorded behavior only for which check fires first: real EMR validates the ServiceRole's assumability and the network objects BEFORE creating anything, and refuses the request with 400 and either `ValidationException` (network objects do not resolve) or `InvalidRequestException` (service role not assumable, 'cannot assume role'); no studio exists afterwards and ListStudios never shows the name.
Emulator: returns HTTP 200 with a fabricated studio — `{'StudioId': 'es-0000000000001', 'Url': 'https://es-0000000000001.emrstudio-prod.us-west-2.amazonaws.com'}` — for a request naming a VpcId/SubnetId/security groups that do not exist and a role that cannot be assumed. (The ids/URL are per-run values; the defect is the 200 and the created resource.)
Why: the whole validate-before-create contract is missing: a caller gets a studio id for a studio that could never work, and the 'rejected CreateStudio leaves nothing behind' invariant cannot be exercised.
Fix: the CreateStudio handler's validation in the EMR crate (create_studio.rs) — resolve VpcId/SubnetIds/WorkspaceSecurityGroupId/EngineSecurityGroupId against EC2 state and the ServiceRole against IAM before allocating a studio, and refuse with 400 rather than persisting one.
Traces: emr.op-create-studio-rejects-unresolvable-network.

--- Cause 3 of 4: ListInstanceFleets omits the fleet's target capacities ---
AWS: Smithy: InstanceFleet members TargetOnDemandCapacity and TargetSpotCapacity are WholeNumber (@range min 0); AWS returns both for every fleet it describes, using 0 for 'none requested' (the docs' own 'If not specified or set to 0 ...' wording presumes the member is present and zero, and ModifyInstanceFleet requires both capacities in one call, so a client must be able to read the current value to hold one side constant).
Emulator: the InstanceFleet entries returned by ListInstanceFleets for a running instance-fleet cluster have no TargetSpotCapacity member at all — `initial_core_fleet['TargetSpotCapacity']` raises KeyError on the emulator's own response.
Why: the modelled read-modify-write cycle for fleet resizing is impossible: any client that reads the current spot capacity to pass it back to ModifyInstanceFleet fails with a missing-key error before it can even issue the call (the check never reaches its ModifyInstanceFleet assertions).
Fix: the ListInstanceFleets response serialization/state projection in the EMR crate (list_instance_fleets.rs and the instance-fleet state it reads) — always emit TargetOnDemandCapacity and TargetSpotCapacity (0 when nothing was requested), as well as the Provisioned* counterparts.
Traces: emr.modify-instance-fleet-terminated-cluster-rejected.

--- Cause 4 of 4: a WAITING cluster reports no MasterPublicDnsName ---
AWS: Smithy: Cluster$MasterPublicDnsName (XmlString) — the DNS name of the cluster's master node (the private DNS name on a private subnet, the public one on a public subnet). Once the master instance is provisioned and the cluster reaches RUNNING/WAITING, DescribeCluster populates it.
Emulator: DescribeCluster on a cluster the emulator itself has driven to WAITING with running instances returns an empty/absent MasterPublicDnsName.
Why: WAITING is precisely the state at which clients connect to the master (SSH, Livy, Spark/Hive endpoints, Zeppelin URLs); with the member absent such a client has no address to use and either crashes on the missing key or polls forever for a value that never appears.
Fix: the cluster state machine / DescribeCluster projection in the EMR crate — populate MasterPublicDnsName from the master instance when the cluster leaves BOOTSTRAPPING, so it is set for every RUNNING/WAITING cluster.
Traces: emr.cluster-reaches-waiting-with-running-instances.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (service emr):

Cause 1 — emr.op-run-job-flow-unknown-release-label-fails: RunJobFlow(Name=<unique>, ReleaseLabel='emr-99.99.0', Instances=..., ServiceRole=<in-check IAM role>, JobFlowRole=<in-check instance profile>) was expected to fail with code ValidationException, HTTP 400 and message 'The supplied release label is invalid: emr-99.99.0.'. AssertionError: 'expected ValidationException, got None: None' — no error was raised, i.e. the submission was accepted.

Cause 2 — emr.op-create-studio-rejects-unresolvable-network: CreateStudio(Name=<unique>, AuthMode='IAM', VpcId='vpc-0fedcba987654321f', SubnetIds=['subnet-0fedcba987654321f'], ServiceRole='arn:aws:iam::<acct>:role/EMR_DefaultRole', WorkspaceSecurityGroupId='sg-0fedcba987654321a', EngineSecurityGroupId='sg-0fedcba987654321b', DefaultS3Location='s3://<unique>/') was expected to return ValidationException or InvalidRequestException. AssertionError: 'expected CreateStudio to reject a bad network/role request, but it SUCCEEDED and returned {"StudioId": "es-0000000000001", "Url": "https://es-0000000000001.emrstudio-prod.us-west-2.amazonaws.com", "ResponseMetadata": {"HTTPStatusCode": 200, ...}}'.

Cause 3 — emr.modify-instance-fleet-terminated-cluster-rejected: after the instance-fleet cluster reached RUNNING/WAITING and the CORE fleet was listed, `spot_capacity = initial_core_fleet['TargetSpotCapacity']` raised KeyError: 'TargetSpotCapacity' on the emulator's ListInstanceFleets response (check line 30), so the operation under test was never reached.

Cause 4 — emr.cluster-reaches-waiting-with-running-instances: AssertionError: 'a WAITING cluster must report MasterPublicDnsName; got an empty/absent value' (check line 294), after the emulator had itself reported the cluster WAITING with running instances.

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1 (service token `emr`):
1. emr.op-run-job-flow-unknown-release-label-fails — create an EMR service role + EC2 instance profile in IAM, then RunJobFlow with ReleaseLabel='emr-99.99.0'; expect HTTP 400/ValidationException with 'The supplied release label is invalid: emr-99.99.0.' and no cluster in ListClusters; the identical call with ReleaseLabel='emr-6.15.0' must still be accepted.
2. emr.op-create-studio-rejects-unresolvable-network — CreateStudio naming a nonexistent vpc-/subnet-/sg- id and the account's EMR_DefaultRole; expect a 400 refusal and no studio in ListStudios.
3. emr.modify-instance-fleet-terminated-cluster-rejected — RunJobFlow an instance-fleet cluster, wait for RUNNING/WAITING, then ListInstanceFleets and read TargetSpotCapacity / TargetOnDemandCapacity off the CORE fleet.
4. emr.cluster-reaches-waiting-with-running-instances — RunJobFlow a cluster, wait until DescribeCluster reports WAITING, then read Cluster.MasterPublicDnsName.
