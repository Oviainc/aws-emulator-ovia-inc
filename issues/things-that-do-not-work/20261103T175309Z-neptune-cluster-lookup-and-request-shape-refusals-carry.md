Issue-Source: trace-sweep-divergence
Issue-Trace: neptune-npt-traj-cluster-lookup-mistakes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: neptune-npt-traj-cluster-lookup-mistakes neptune-cluster-parameter-group-lifecycle neptune-npt-traj-create-cluster-request-shape
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune|DescribeDBClusters|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: neptune|DescribeDBClusters|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|DescribeDBClusters|4xx->4xx|status
Issue-Key: neptune|ModifyDBClusterParameterGroup|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: neptune|CreateDBCluster|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Commit: c27f8675843924c63f0ee6aedd1be01f08149ed1

# Neptune cluster-lookup and request-shape refusals carry the wrong status, error code and message template

> _Note: this issue was identified against the code base at commit `c27f8675843924c63f0ee6aedd1be01f08149ed1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
The Neptune front door's refusal paths for cluster lookups, filter names, request-shape errors and parameter-value validation are produced by a generic validator that picks a different error class from AWS's and words its messages differently — in one case turning a 400 input-validation refusal into a 404 not-found. Five clusters, all reported with owned_by=[] and stub_path="". The related open issue 20261029T174713Z-neptune-refusals-are-worded-and-classified-differently-from.md owns a DIFFERENT set of refusal clusters (subnet-group deletion/coverage, snapshot-already-exists, invalid DB engine on CreateDBCluster, cluster-parameter-group name validation, event-category lookup, missing subnet group on CreateDBCluster); replay reports every cluster below with an empty owned_by, so none of them is carried by it. THIS ISSUE HAS 5 CAUSES — all five must be closed before submitting.

Cause 1 of 5: DescribeDBClusters treats a syntactically invalid cluster identifier as a missing cluster. AWS: the only error DescribeDBClusters models is DBClusterNotFoundFault, httpError 404, awsQuery code 'DBClusterNotFoundFault' (Smithy: DescribeDBClusters errors=[DBClusterNotFoundFault]) — that fault is for an identifier that is WELL FORMED but names nothing. An identifier that cannot be a cluster identifier at all is rejected as input: the recording shows HTTP 400 with Error Type 'Sender', Code 'InvalidParameterValue' and Message 'Invalid database cluster identifier: ' + the offending identifier from the request (note AWS's template leaves a single space after the colon before the value). Emulator: HTTP 404 with Code 'DBClusterNotFoundFault' and Message 'DBCluster <identifier> not found.'. Why: the two classes mean different things to a caller — 404/not-found invites a create-or-retry, whereas 400/InvalidParameterValue says the identifier can never be valid and the request must be corrected; a client that provisions on not-found will loop. Fix: the DescribeDBClusters handler's identifier validation, which must run before the state lookup and raise InvalidParameterValue/400. Traces: neptune-npt-traj-cluster-lookup-mistakes#task#1.

Cause 2 of 5: the DBClusterNotFoundFault message echoes the full ARN the caller passed instead of the cluster identifier. AWS: when DBClusterIdentifier is given as a cluster ARN that resolves to nothing, the service answers 404 DBClusterNotFoundFault with Message 'DBCluster ' + the cluster IDENTIFIER extracted from the ARN + ' not found.' — the fixed text is 'DBCluster ' and ' not found.', and the variable part is the identifier, not the ARN as supplied. Emulator: the message interpolates the entire ARN ('DBCluster arn:aws:rds:<region>:<account>:cluster:<identifier> not found.'). Why: the status and code are right, but the message is the only place the offending resource is named, and a client or operator comparing it against the service's output (log assertions, surfaced error text) sees a different value. Fix: the DBClusterNotFoundFault message construction in the DescribeDBClusters handler — normalise an ARN-form identifier to its identifier before interpolating it. Traces: neptune-npt-traj-cluster-lookup-mistakes#task#2.

Cause 3 of 5: the unrecognised-filter refusal appends a supported-filter listing AWS does not emit. AWS: Filters is a FilterList on DescribeDBClustersMessage; an unsupported filter name is refused with HTTP 400 InvalidParameterValue and Message 'Unrecognized filter name: ' + the filter name from the request — that is the whole message. Emulator: 'unrecognized filter name: <name>; supported filters are clone-group-id, db-cluster-id, db-cluster-resource-id, domain, engine' — lower-cased first word and an appended enumeration of the filters it happens to implement. Why: besides not matching the service's text, the appended list advertises the emulator's own filter support as if it were AWS's, so a client that reads it builds requests against the wrong filter set. Fix: the message template used by the DescribeDBClusters filter validation. Traces: neptune-npt-traj-cluster-lookup-mistakes#task#4.

Cause 4 of 5: CreateDBCluster reports a malformed/invalid parameter value as MalformedInput rather than InvalidParameterValue. AWS: for a request whose parameter VALUE is unacceptable, the recorded refusal is HTTP 400 with Error Type 'Sender' and Code 'InvalidParameterValue'; MalformedInput is the awsQuery protocol's class for a request body that cannot be parsed at all, which is not what happened here (the status and the rest of the envelope match). Emulator: HTTP 400 with Code 'MalformedInput'. Why: Code is what SDK error handling and client retry/branch logic key on; MalformedInput reads as a protocol-level framing failure rather than a rejected parameter value, so a caller cannot map the refusal to the member it must fix. Fix: the CreateDBCluster handler's input validation — value-level rejections must be raised as InvalidParameterValue, reserving MalformedInput for unparseable bodies. Traces: neptune-npt-traj-create-cluster-request-shape#task#1.

Cause 5 of 5: ModifyDBClusterParameterGroup words a rejected boolean parameter value with its own allowed-values explanation. AWS: Parameters is a required ParametersList on ModifyDBClusterParameterGroupMessage (Smithy: ModifyDBClusterParameterGroup, input Parameters required); a value that is not a boolean for a parameter whose neptune#Parameter DataType is 'boolean' is refused with HTTP 400 InvalidParameterValue and Message 'Invalid boolean value: ' + the value supplied in the request — fixed text 'Invalid boolean value: ' plus that value, naming neither the parameter nor its allowed values. Emulator: HTTP 400 with Message 'Invalid parameter value: <value> for: <parameter name> allowed values are: 0,1' — a different template carrying the parameter name and the allowed-value list. Why: status and code match, wording does not; a client surfacing or matching the service's refusal text diverges, and the emulator's extra detail is a contract it invents. Fix: the message template used by ModifyDBClusterParameterGroup's parameter-value validation. Traces: neptune-cluster-parameter-group-lifecycle#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 181 steps, 110 passed, 69 mismatched, score 60.8%, 45 divergence clusters, all service=neptune. The five clusters below carry owned_by=[] and stub_path="".

Cause 1 — neptune-npt-traj-cluster-lookup-mistakes#task#1, DescribeDBClusters, expected 400 / actual 404, verdict mismatch. divergence_key service=neptune|DescribeDBClusters|status=400->404. status: expected 400, got 404; $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'DBClusterNotFoundFault'); $.ErrorResponse.Error.Message: value_mismatch (expected 'Invalid database cluster identifier:  Bad_Name!', got 'DBCluster Bad_Name! not found.').

Cause 2 — neptune-npt-traj-cluster-lookup-mistakes#task#2, DescribeDBClusters, 404 / 404. $.ErrorResponse.Error.Message: value_mismatch (expected 'DBCluster <cluster-identifier> not found.', got 'DBCluster arn:aws:rds:us-west-2:000000000000:cluster:<cluster-identifier> not found.').

Cause 3 — neptune-npt-traj-cluster-lookup-mistakes#task#4, DescribeDBClusters, 400 / 400. $.ErrorResponse.Error.Message: value_mismatch (expected 'Unrecognized filter name: cluster-id', got 'unrecognized filter name: cluster-id; supported filters are clone-group-id, db-cluster-id, db-cluster-resource-id, domain, engine').

Cause 4 — neptune-npt-traj-create-cluster-request-shape#task#1, CreateDBCluster, 400 / 400. $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidParameterValue', got 'MalformedInput') — status and message envelope otherwise match.

Cause 5 — neptune-cluster-parameter-group-lifecycle#task#9, ModifyDBClusterParameterGroup, 400 / 400. $.ErrorResponse.Error.Message: value_mismatch (expected 'Invalid boolean value: yes', got 'Invalid parameter value: yes for: neptune_enable_audit_log allowed values are: 0,1').

## Steps to Reproduce
1. Clone the repo, boot the emulator built from it, and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay neptune-npt-traj-cluster-lookup-mistakes step task#1 (DescribeDBClusters with a DBClusterIdentifier that cannot be a valid identifier) and observe 404 DBClusterNotFoundFault where AWS answers 400 InvalidParameterValue 'Invalid database cluster identifier:  <value>'.
3. Cause 2: replay the same fixture step task#2 (DescribeDBClusters with a cluster ARN for a cluster that does not exist) and compare the Error.Message — it must name the cluster identifier, not the ARN.
4. Cause 3: replay the same fixture step task#4 (DescribeDBClusters with an unsupported Filters[].Name) and compare the Error.Message against 'Unrecognized filter name: <name>'.
5. Cause 4: replay neptune-npt-traj-create-cluster-request-shape step task#1 (CreateDBCluster with an unacceptable parameter value) and compare Error.Code — InvalidParameterValue, not MalformedInput.
6. Cause 5: replay neptune-cluster-parameter-group-lifecycle step task#9 (ModifyDBClusterParameterGroup setting a boolean parameter to a non-boolean value) and compare Error.Message against 'Invalid boolean value: <value>'.
