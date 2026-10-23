Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-create-cluster-missing-subnet-group-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-create-cluster-missing-subnet-group-error redshift-parameter-group-lifecycle redshift-empty-inventory-reads
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|CreateCluster|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateCluster|4xx->2xx|status
Issue-Key: redshift|DescribeClusters|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeClusters|4xx->2xx|status
Issue-Key: redshift|DescribeClusterParameterGroups|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeClusterParameterGroups|4xx->2xx|status
Issue-Key: redshift|DescribeStorage|2xx->4xx|$.DescribeStorageResponse
Issue-Key: redshift|DescribeStorage|2xx->4xx|status
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# Redshift control plane skips existence preconditions: CreateCluster/DescribeClusters/DescribeClusterParameterGroups return 200 where AWS errors, DescribeStorage returns 400

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Four distinct causes, all of them HTTP-status divergences in the Redshift control plane: three operations succeed where AWS raises a modeled error, and one operation rejects a request AWS accepts. A client cannot distinguish success from failure on these paths at all.

Cause 1 of 4 — CreateCluster does not validate ClusterSubnetGroupName (fixture redshift-create-cluster-missing-subnet-group-error, step task#1).
AWS: CreateCluster takes an optional ClusterSubnetGroupName and models ClusterSubnetGroupNotFoundFault among its errors (Smithy: CreateCluster errors include ClusterSubnetGroupNotFoundFault, errorKind client, httpError 400, awsQueryError code ClusterSubnetGroupNotFoundFault / httpResponseCode 400). When the named subnet group does not exist in the account, the create is refused before any cluster is allocated: the recording shows HTTP 400 with ErrorResponse.Error.Type 'Sender' and Code 'ClusterSubnetGroupNotFoundFault' (recorded message, as an example only: "Cluster subnet group '<name>' does not exist.").
Emulator: returns HTTP 200 with a CreateClusterResponse and no ErrorResponse element at all — the unknown subnet group is accepted and a cluster is created.
Why: a client that creates a cluster against a mistyped or not-yet-created subnet group is told the cluster exists. It then waits for a cluster that would never exist on AWS, and any error handling bound to ClusterSubnetGroupNotFoundFault is dead code. Worse, the emulator's state now holds a cluster AWS never created, so every later read diverges too (cause 2).
Fix: CreateCluster's request validation in the redshift service crate — resolve ClusterSubnetGroupName against subnet-group state before admitting the cluster, and raise the modeled ClusterSubnetGroupNotFoundFault (400) when it does not resolve.

Cause 2 of 4 — DescribeClusters returns 200 for a cluster identifier that does not exist (fixture redshift-create-cluster-missing-subnet-group-error, step task#2).
AWS: Smithy: DescribeClusters errors include ClusterNotFoundFault, errorKind client, httpError 404, awsQueryError code ClusterNotFound / httpResponseCode 404. A DescribeClusters naming an identifier with no cluster behind it is a 404 ClusterNotFound, Error.Type 'Sender'.
Emulator: HTTP 200 with a cluster body and no ErrorResponse.
Why: a not-found read is reported as a hit, so a client polling for a cluster that was never successfully created never learns it must retry or fail. In this fixture the admitted cluster is the one cause 1 wrongly created, which is why the two travel together; the handler must still raise ClusterNotFound for an unknown identifier independently of cause 1.
Fix: DescribeClusters handler/state lookup — when the requested ClusterIdentifier is absent from state, raise ClusterNotFoundFault (404) instead of synthesizing a result.

Cause 3 of 4 — DescribeClusterParameterGroups returns 200 for a parameter group that was deleted (fixture redshift-parameter-group-lifecycle, step teardown#12).
AWS: Smithy: DescribeClusterParameterGroups errors include ClusterParameterGroupNotFoundFault, errorKind client, httpError 404, awsQueryError code ClusterParameterGroupNotFound / httpResponseCode 404. After a successful DeleteClusterParameterGroup, describing that group by name is a 404 with Code 'ClusterParameterGroupNotFound' and Error.Type 'Sender'.
Emulator: HTTP 200, no ErrorResponse — the group is still visible after deletion.
Why: deletion is not observable. A client that deletes a parameter group and confirms by describing it concludes the delete failed (or, in the opposite reading, that a deleted resource is still usable), and teardown/idempotency logic that depends on the 404 never fires.
Fix: DeleteClusterParameterGroup's state mutation and/or the DescribeClusterParameterGroups lookup — the delete must remove the group from state, and a by-name describe of an absent group must raise ClusterParameterGroupNotFoundFault (404).

Cause 4 of 4 — DescribeStorage rejects a valid request with 400 (fixture redshift-empty-inventory-reads, step task#17).
AWS: Smithy: DescribeStorage takes Unit input (no members) and returns CustomerStorageMessage with TotalBackupSizeInMegaBytes and TotalProvisionedStorageInMegaBytes (both Double); it models NO errors at all. On an account with nothing provisioned the recording is HTTP 200 with DescribeStorageResult carrying both doubles as '0.0'.
Emulator: HTTP 400. The operation is not reported as an unimplemented stub (stub_path empty), so a request that cannot be invalid — it has no input members to get wrong — is being refused.
Why: an input-less, error-less read fails. Any client or console view that opens with a storage summary errors out on an empty account, and because DescribeStorage models no errors the SDK surfaces this as an unmodeled/unknown fault rather than something a caller can handle.
Fix: the DescribeStorage handler in the redshift service crate — serve CustomerStorageMessage with zeroed doubles when no clusters or snapshots exist rather than returning a 400.

Traces: redshift-create-cluster-missing-subnet-group-error#task#1 (cause 1), redshift-create-cluster-missing-subnet-group-error#task#2 (cause 2), redshift-parameter-group-lifecycle#teardown#12 (cause 3), redshift-empty-inventory-reads#task#17 (cause 4). All four must be closed before submitting. No open issue covers these: the existing open redshift issues are all redshift-data / SQL-engine scoped.

## Evidence
Cause 1 — redshift-create-cluster-missing-subnet-group-error#task#1, CreateCluster. Expected status 400, actual 200. Replay detail: "status: expected 400, got 200" and "$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ClusterSubnetGroupNotFoundFault', 'Message': \"Cluster subnet group '<name>' does not exist.\"}, ...}, got None)". Requirement: status 400 + Code ClusterSubnetGroupNotFoundFault + Type Sender; recorded Message is an example.

Cause 2 — redshift-create-cluster-missing-subnet-group-error#task#2, DescribeClusters. Expected status 404, actual 200. Replay detail: "status: expected 404, got 200" and "$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ClusterNotFound', ...}}, got None)". Requirement: status 404 + Code ClusterNotFound.

Cause 3 — redshift-parameter-group-lifecycle#teardown#12, DescribeClusterParameterGroups. Expected status 404, actual 200. Replay detail: "status: expected 404, got 200" and "$.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'ClusterParameterGroupNotFound', ...}}, got None)". Requirement: status 404 + Code ClusterParameterGroupNotFound.

Cause 4 — redshift-empty-inventory-reads#task#17, DescribeStorage. Expected status 200, actual 400. Replay detail: "status: expected 200, got 400" and "$.DescribeStorageResponse: missing (expected {'DescribeStorageResult': {'TotalBackupSizeInMegaBytes': '0.0', 'TotalProvisionedStorageInMegaBytes': '0.0'}, ...}, got None)". Model: DescribeStorage input Unit, output CustomerStorageMessage, errors [] — no input can make it a client error.

Model citations: Smithy CreateCluster -> ClusterSubnetGroupNotFoundFault (400, awsQueryError ClusterSubnetGroupNotFoundFault); DescribeClusters -> ClusterNotFoundFault (404, awsQueryError ClusterNotFound); DescribeClusterParameterGroups -> ClusterParameterGroupNotFoundFault (404, awsQueryError ClusterParameterGroupNotFound); DescribeStorage input Unit / output CustomerStorageMessage / errors [].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture redshift-create-cluster-missing-subnet-group-error step task#1 — CreateCluster with ClusterSubnetGroupName naming a subnet group that was never created. Expect HTTP 400 ErrorResponse Code ClusterSubnetGroupNotFoundFault; observe HTTP 200 CreateClusterResponse.
3. Cause 2: same fixture, step task#2 — DescribeClusters for the ClusterIdentifier from step task#1. Expect HTTP 404 Code ClusterNotFound; observe HTTP 200 with a cluster.
4. Cause 3: replay fixture redshift-parameter-group-lifecycle through teardown#12 — CreateClusterParameterGroup, DeleteClusterParameterGroup, then DescribeClusterParameterGroups for that name. Expect HTTP 404 Code ClusterParameterGroupNotFound; observe HTTP 200.
5. Cause 4: replay fixture redshift-empty-inventory-reads step task#17 — DescribeStorage with no parameters against an account with no clusters. Expect HTTP 200 with TotalBackupSizeInMegaBytes/TotalProvisionedStorageInMegaBytes; observe HTTP 400.
