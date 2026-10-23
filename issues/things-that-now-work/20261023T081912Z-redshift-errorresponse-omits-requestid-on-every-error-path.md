Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-cluster-lifecycle-op-preconditions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-cluster-lifecycle-op-preconditions redshift-integration-and-idc-application-preconditions redshift-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|ModifyCluster|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|ModifyCluster|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: redshift|DeleteCluster|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: redshift|CreateIntegration|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: redshift|CreateClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateClusterParameterGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: redshift|DescribeClusterParameters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|DescribeClusterParameters|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# Redshift ErrorResponse omits RequestId on every error path

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCluster

## Description
Five causes with one shared fault: the Redshift awsQuery error serializer emits ErrorResponse.Error but no sibling ErrorResponse.RequestId. AWS's query-protocol error envelope for Redshift is <ErrorResponse><Error>...</Error><RequestId>...</RequestId></ErrorResponse>, and every recorded error in this corpus carries a RequestId. The status and error code are correct on all five — only the envelope member is absent — so this is one serializer defect observed on five operations. Fix it once in the error serializer and all five close.

The RequestId VALUE is environment-specific and is not a requirement; its PRESENCE as a member of ErrorResponse is. Where a cause below also shows an error-message text difference, the recorded text is noted as an example only: these are not-found/already-exists messages whose exact template AWS does not document and the model does not state, so the requirement is the status and the code, not the wording.

Cause 1 of 5 — ModifyCluster against an absent cluster (fixture redshift-cluster-lifecycle-op-preconditions, step task#1).
AWS: Smithy: ModifyCluster errors include ClusterNotFoundFault, errorKind client, httpError 404, awsQueryError code ClusterNotFound / httpResponseCode 404. Recording: HTTP 404, ErrorResponse.Error.Code ClusterNotFound, plus ErrorResponse.RequestId.
Emulator: HTTP 404 and the correct code, but ErrorResponse.RequestId is missing. (The message text also differs — recorded "Cluster not found: <id>" vs emulator "Cluster <id> not found."; both name the requested identifier, and the template is not a modeled requirement, so this is an example, not the defect.)
Why: see shared impact below.

Cause 2 of 5 — DeleteCluster against an absent cluster (fixture redshift-cluster-lifecycle-op-preconditions, step task#2).
AWS: Smithy: DeleteCluster errors include ClusterNotFoundFault (404, awsQueryError ClusterNotFound). Recording: HTTP 404 with ErrorResponse.RequestId present.
Emulator: HTTP 404 with the right code; ErrorResponse.RequestId missing. This cause is the clean one — RequestId is the ONLY divergence, which is what identifies the shared root cause as the error serializer rather than any one handler.

Cause 3 of 5 — CreateIntegration precondition failure (fixture redshift-integration-and-idc-application-preconditions, step task#1).
AWS: recorded behavior: HTTP 400 ErrorResponse for the precondition this step violates, with ErrorResponse.RequestId present.
Emulator: HTTP 400 with the expected Error body; ErrorResponse.RequestId missing.

Cause 4 of 5 — CreateClusterParameterGroup for a name that already exists (fixture redshift-parameter-group-lifecycle, step task#8).
AWS: Smithy: ClusterParameterGroupAlreadyExistsFault is the modeled already-exists error for this operation. Recording: HTTP 400 with ErrorResponse.RequestId present.
Emulator: HTTP 400, correct code; ErrorResponse.RequestId missing. (Message template also differs — recorded "Parameter group <name> already exists" vs emulator "Cluster parameter group already exists: <name>"; example only.)

Cause 5 of 5 — DescribeClusterParameters for an absent parameter group (fixture redshift-parameter-group-lifecycle, step task#9).
AWS: Smithy: DescribeClusterParameters errors include ClusterParameterGroupNotFoundFault, errorKind client, httpError 404, awsQueryError code ClusterParameterGroupNotFound / httpResponseCode 404. Recording: HTTP 404 with ErrorResponse.RequestId present.
Emulator: HTTP 404, correct code; ErrorResponse.RequestId missing. (Message template also differs — recorded "ClusterParameterGroup not found: <name>" vs emulator "Cluster parameter group not found: <name>"; example only.)

Why (shared): the query-protocol RequestId is the only handle a caller has on a failed call. SDK retry/debug layers surface it as the request id of the error, tooling logs it, and support flows quote it; with the member absent the SDK reports an error whose request id is null while the successful responses in the same fixture do carry one. Any client or test that asserts on the error envelope being well formed — or that correlates a failure back to a request — breaks on every Redshift error path, not just these five operations.
Fix (shared): the Redshift awsQuery error serializer in the redshift service crate — emit RequestId as a sibling of Error inside ErrorResponse for every error response, taking the value from the per-request id already used on the success path.

Traces: redshift-cluster-lifecycle-op-preconditions#task#1 (cause 1), redshift-cluster-lifecycle-op-preconditions#task#2 (cause 2), redshift-integration-and-idc-application-preconditions#task#1 (cause 3), redshift-parameter-group-lifecycle#task#8 (cause 4), redshift-parameter-group-lifecycle#task#9 (cause 5). All five must be closed before submitting; one serializer change should close them together. Note also that two further 404s in this corpus (redshift-snapshot-op-preconditions-and-batch-results CreateClusterSnapshot and RestoreFromClusterSnapshot) differ ONLY in message wording with correct status and code, and are deliberately NOT filed as causes here.

## Evidence
Cause 1 — redshift-cluster-lifecycle-op-preconditions#task#1, ModifyCluster, expected 404 / actual 404. Replay detail: "$.ErrorResponse.RequestId: missing (expected '870e3f8d-b83d-4771-84b9-64e871025022', got None)" and "$.ErrorResponse.Error.Message: value_mismatch (expected 'Cluster not found: sim2real-probe-cluster-missing', got 'Cluster sim2real-probe-cluster-missing not found.')".

Cause 2 — redshift-cluster-lifecycle-op-preconditions#task#2, DeleteCluster, expected 404 / actual 404. Replay detail: "$.ErrorResponse.RequestId: missing (expected '266ceaf4-1494-4d3c-be80-dd339f0cd8f0', got None)" — RequestId is the ONLY divergence on this step.

Cause 3 — redshift-integration-and-idc-application-preconditions#task#1, CreateIntegration, expected 400 / actual 400. Replay detail: "$.ErrorResponse.RequestId: missing (expected '33aaf597-510d-4e43-96ff-238cb018cc8a', got None)".

Cause 4 — redshift-parameter-group-lifecycle#task#8, CreateClusterParameterGroup, expected 400 / actual 400. Replay detail: "$.ErrorResponse.RequestId: missing (expected '3e0d1d9b-8fef-4236-9c5e-0edce644f85b', got None)" plus the message-template difference quoted above.

Cause 5 — redshift-parameter-group-lifecycle#task#9, DescribeClusterParameters, expected 404 / actual 404. Replay detail: "$.ErrorResponse.RequestId: missing (expected '60aa1c72-6d06-40f7-ba73-ec0bf594a6a6', got None)" plus the message-template difference quoted above.

The quoted UUIDs are the recorded examples — the requirement is that ErrorResponse carries a RequestId member, not any particular value. Model citations: Smithy ModifyCluster/DeleteCluster -> ClusterNotFoundFault (404, awsQueryError ClusterNotFound); DescribeClusterParameters -> ClusterParameterGroupNotFoundFault (404, awsQueryError ClusterParameterGroupNotFound).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 2 is the simplest probe: replay fixture redshift-cluster-lifecycle-op-preconditions step task#2 — DeleteCluster with a ClusterIdentifier that does not exist. The 404 and Code ClusterNotFound are correct; inspect the XML body and observe <ErrorResponse> contains <Error> but no <RequestId>.
3. Cause 1: same fixture, step task#1 — ModifyCluster on the same absent identifier; same missing RequestId.
4. Cause 3: replay fixture redshift-integration-and-idc-application-preconditions step task#1 — CreateIntegration violating its precondition; 400 body lacks RequestId.
5. Causes 4 and 5: replay fixture redshift-parameter-group-lifecycle to step task#8 (CreateClusterParameterGroup re-creating an existing group, 400) and step task#9 (DescribeClusterParameters for an absent group, 404); both bodies lack RequestId.
6. Compare against any successful step in the same fixtures, which does carry a response request id.
