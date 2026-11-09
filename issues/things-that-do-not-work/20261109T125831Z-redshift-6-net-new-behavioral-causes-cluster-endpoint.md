Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.deploy-redshift-cluster-cfn-behavioral-connectivity redshift.get-cluster-credentials-temporary-scoped-access redshift.redshift-sql-connectivity redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows redshift.rsql-ddl-dml-readback-visible-to-a-second-session redshift.rsql-distkey-changes-the-join-plan redshift.rsql-not-null-violation-rejected redshift.rsql-transaction-rollback-leaves-no-row redshift.namespace-invalid-state-and-namespace-errors redshift.namespace-register-wrong-state-blocked redshift.parameter-group-known-parameter-value-validation redshift.data-api-statement-id-and-target-validation-rejected redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected
Issue-Key: redshift.deploy-redshift-cluster-cfn-behavioral-connectivity|run#19|PublishedUrlRejected|endpoint_address_dial, endpoint_port_dial = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.get-cluster-credentials-temporary-scoped-access|run#18|PublishedUrlRejected|address, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.redshift-sql-connectivity|run#15|PublishedUrlRejected|*ctx.endpoint_host(endpoint["Address"], endpoint["Port"]),
Issue-Key: redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer|run#19|PublishedUrlRejected|endpoint_address_dial, endpoint_port_dial = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent|_connect#6|PublishedUrlRejected|host, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows|_connect#6|PublishedUrlRejected|host, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.rsql-ddl-dml-readback-visible-to-a-second-session|_connect#6|PublishedUrlRejected|host, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.rsql-distkey-changes-the-join-plan|run#5|PublishedUrlRejected|address, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.rsql-not-null-violation-rejected|_connect#6|PublishedUrlRejected|host, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.rsql-transaction-rollback-leaves-no-row|run#5|PublishedUrlRejected|address, port = ctx.endpoint_host(endpoint["Address"], int(endpoint["Port"]))
Issue-Key: redshift.namespace-invalid-state-and-namespace-errors|run#11|AssertionError|assert precondition_code == "InvalidParameterValue", (
Issue-Key: redshift.namespace-register-wrong-state-blocked|run#8|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: redshift.parameter-group-known-parameter-value-validation|run#15|AssertionError|assert message == "parameter value is not a valid boolean:maybe", message
Issue-Key: redshift.data-api-statement-id-and-target-validation-rejected|run#14|AssertionError|assert message == "Redshift endpoint doesn't exist in this region.", message
Issue-Key: redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll|_run_sql#23|AssertionError|assert described["Status"] == "FINISHED", (
Issue-Key: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected|run#9|AssertionError|assert running["Status"] == "STARTED", (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# Redshift: 6 net-new behavioral causes — cluster Endpoint.Address is an AWS-owned host the emulator does not serve, RegisterNamespace/parameter-value/Data-API refusals are emulator-authored, and long statements die 'interrupted'

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
Six net-new root causes on the Redshift surface, all grounded in failing behavioral checks of this replay. Each cause carries its own failing check ids; all six must be closed.

Cause 1 of 6 — an available cluster publishes an Endpoint.Address on AWS's own domain, which no customer of the emulator can reach.
AWS: Smithy: DescribeClusters/CreateCluster output Cluster.Endpoint -> Endpoint{Address: String, Port: Integer}; Amazon Redshift speaks the PostgreSQL wire protocol and a client opens a session to that Address:Port (default 5439) once ClusterStatus is 'available' (the wire data plane itself is not modeled — recorded behavior only: the published address accepts a PostgreSQL session shortly after the cluster reports available).
Emulator: Endpoint.Address is published as <cluster-identifier>.<12-hex>.<region>.redshift.amazonaws.com with Port 5439 — a real-AWS hostname that is not an address the emulator serves, so the published target is refused before any socket is opened: 'simparity.published.PublishedUrlRejected: endpoint_host was given <host>:5439 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address'. The fixed part is the rejection class and reason; the host label and the cluster id in it are this run's generated values.
Why: a client that creates a cluster and connects to the endpoint the API handed it is pointed at AWS's real domain instead of the emulator. The whole native data plane — psql/psycopg/JDBC sessions, GetClusterCredentials logins, DDL/DML round-trips, transaction and NOT NULL semantics, distribution-key plans — is unreachable even though the control plane reports a usable warehouse.
Fix: the cluster projection/state that builds Cluster.Endpoint (the DescribeClusters/CreateCluster response serializer and the cluster state that stores the endpoint) must publish an address on the emulator's own listener, and the PostgreSQL-wire listener must answer there; the endpoint must not be advertised before it answers.
Traces: redshift.deploy-redshift-cluster-cfn-behavioral-connectivity, redshift.get-cluster-credentials-temporary-scoped-access, redshift.redshift-sql-connectivity, redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer, redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent, redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows, redshift.rsql-ddl-dml-readback-visible-to-a-second-session, redshift.rsql-distkey-changes-the-join-plan, redshift.rsql-not-null-violation-rejected, redshift.rsql-transaction-rollback-leaves-no-row. Related, not covering: open issue 20261104T045922Z-redshift-publishes-a-cluster-endpoint-but-serves-no.md records the same checks from an earlier run where the endpoint was a loopback address with nothing listening; the published host is now an amazonaws.com name and the refusal lands before any connect, so that issue's replay keys no longer match these failures.

Cause 2 of 6 — RegisterNamespace against a cluster that has not finished initializing answers the wrong error class and an emulator-authored message.
AWS: Smithy: RegisterNamespace errors are ClusterNotFoundFault (404, awsQueryError ClusterNotFound), InvalidClusterStateFault (400, InvalidClusterState) and InvalidNamespaceFault (400). Recorded behavior: a cluster that exists but is still 'creating' is refused with the service-common query error InvalidParameterValue (400) whose message is the fixed text 'This cluster version doesn't support Register Namespace operation. Please upgrade to the latest cluster version.' — the message has no variable part.
Emulator: answers InvalidClusterState with 'Cluster <cluster-identifier> is not in available state.' ('Cluster ' + the request's ClusterIdentifier + ' is not in available state.' — the identifier is the only variable part, and the whole template is emulator-authored for this operation).
Why: a client that branches on InvalidParameterValue to detect an unsupported/not-yet-ready namespace registration sees a cluster-state fault instead, so its retry/upgrade path never runs; the message it logs is not AWS's.
Fix: RegisterNamespace's handler validation — the not-available precondition must raise the InvalidParameterValue query error with AWS's fixed message instead of InvalidClusterStateFault.
Traces: redshift.namespace-invalid-state-and-namespace-errors, redshift.namespace-register-wrong-state-blocked.

Cause 3 of 6 — ModifyClusterParameterGroup's boolean-domain refusal carries the emulator's wire-location template instead of AWS's parameter-value message.
AWS: Smithy: ModifyClusterParameterGroup(ParameterGroupName, Parameters: ParametersList); the modeled errors are ClusterParameterGroupNotFoundFault and InvalidClusterParameterGroupStateFault, so a bad value is refused with the service-common InvalidParameterValue (400). Recorded behavior: a boolean parameter given a non-boolean value is refused with code InvalidParameterValue and message 'parameter value is not a valid boolean:<value>' — fixed text 'parameter value is not a valid boolean:' plus the rejected ParameterValue from the request.
Emulator: code is InvalidParameterValue but the message is 'Invalid value for Parameters.Parameter.<n>.ParameterValue: '<value>' is not a value parameter '<name>' allows. Allowed values: true,false' — a wire-location/allowed-values template AWS never emits for this refusal.
Why: a client that matches or surfaces AWS's message for an invalid parameter value gets different prose, and the sibling legs of the same contract ('Invalid JSON for wlm_json_configuration', 'invalid integer value:<value>', 'The statement_timeout of <n> is outside of the acceptable range of 0,100-2147483647') are never reached because the first leg already diverges.
Fix: ModifyClusterParameterGroup's parameter-domain validation — the per-datatype refusal messages, not the generic request-shape validator.
Traces: redshift.parameter-group-known-parameter-value-validation. Related, not covering: open issue 20261106T225316Z-redshift-modifyclusterparametergroup-boolean-value-refusal-invalidparametervalue-message-is-the.md carries only the parameter-group-non-boolean-value-rejected check.

Cause 4 of 6 — a Data API ExecuteStatement naming BOTH ClusterIdentifier and WorkgroupName, where the cluster does not exist, reports the mutual-exclusivity message instead of the endpoint-not-found refusal.
AWS: recorded behavior only (the precedence is not modeled): ExecuteStatement with both targets set resolves the named Redshift endpoint first and refuses with ValidationException whose message is the fixed text 'Redshift endpoint doesn't exist in this region.' The mutual-exclusivity message 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId' is the refusal for a request that supplies no resolvable target, not for one whose named cluster is absent.
Emulator: answers ValidationException with the mutual-exclusivity message for the both-targets/absent-cluster request, collapsing the two validation classes into one.
Why: a client cannot distinguish 'you addressed a cluster that does not exist' from 'you addressed two targets at once', so it retries the same unresolvable endpoint instead of correcting the identifier.
Fix: the redshift-data ExecuteStatement/BatchExecuteStatement handler's target-resolution ordering and its ValidationException messages.
Traces: redshift.data-api-statement-id-and-target-validation-rejected. Related, not covering: open issue 20261105T195000Z-redshift-data-executestatement-target-validation-reports-the-mutual.md records this precedence for the redshift-data service and does not carry this check.

Cause 5 of 6 — a long-running Data API SELECT is terminated FAILED with error 'interrupted' instead of running to FINISHED.
AWS: recorded behavior only: ExecuteStatement on an available cluster runs an expensive but legal statement (a self-join of a seeded table with an always-false predicate) to completion — DescribeStatement eventually reports Status 'FINISHED' with no Error, and the cost is what makes WaitTimeSeconds observable.
Emulator: DescribeStatement settles Status 'FAILED' with Error 'interrupted' for 'SELECT count(*) FROM <table> a, <table> b WHERE a.n + b.n = -1' (table name and predicate constant are the check's values; the behavior is that the engine aborts the statement rather than executing it), so no terminal-without-a-poll result is ever produced.
Why: any client submitting a statement whose execution is not instantaneous gets a spurious failure, and the documented wait-for-terminal-status behavior cannot be used at all.
Fix: the Data API statement executor/engine in the cluster SQL plane — a statement must run to a terminal FINISHED rather than being interrupted by an internal budget.
Traces: redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll.

Cause 6 of 6 — a long INSERT reaches a terminal FAILED before a CancelStatement can land, so cancellation is unobservable.
AWS: recorded behavior only: an INSERT built from a three-way self-join of a seeded 1,000-row table stays in Status 'STARTED' long enough for a concurrent CancelStatement to abort it (and a second CancelStatement on the already-aborted statement is then refused).
Emulator: DescribeStatement reports the statement already 'FAILED' while it must still be running, so the cancel never applies to a running statement and neither the abort nor the second-cancel refusal is exercised.
Why: a client that submits long work and cancels it cannot observe either outcome — the statement fails on its own, and the cancel path and its repeat-cancel refusal are dead code.
Fix: the Data API statement executor/state machine — a submitted statement must remain STARTED while it executes and only leave that state on completion or cancellation.
Traces: redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (redshift: 87/186 passed).

Cause 1 (10 checks, identical refusal) — e.g. redshift.redshift-sql-connectivity: 'simparity.published.PublishedUrlRejected: endpoint_host was given sim2real-behav-rs-sql-b3f37c4481c9.97eb3cacfd8c.us-west-2.redshift.amazonaws.com:5439 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address'. The same rejection, differing only in the generated cluster id and the hex label, is raised for redshift.deploy-redshift-cluster-cfn-behavioral-connectivity, redshift.get-cluster-credentials-temporary-scoped-access, redshift.redshift-get-cluster-credentials-outside-the-requested-dbgroup-is-denied-at-the-sql-layer, redshift.redshift-native-sql-null-into-a-not-null-column-rejected-and-the-row-absent, redshift.redshift-native-sql-rollback-leaves-only-the-committed-rows, redshift.rsql-ddl-dml-readback-visible-to-a-second-session, redshift.rsql-distkey-changes-the-join-plan, redshift.rsql-not-null-violation-rejected, redshift.rsql-transaction-rollback-leaves-no-row. Expected: a host the emulator serves. Actual: an *.redshift.amazonaws.com host.

Cause 2 — redshift.namespace-invalid-state-and-namespace-errors: 'AssertionError: expected InvalidParameterValue, got InvalidClusterState: Cluster sim2real-behav-ns-c3756f0bdc86 is not in available state.' and redshift.namespace-register-wrong-state-blocked: 'AssertionError: expected InvalidParameterValue while the cluster is still creating, got InvalidClusterState'. Expected code InvalidParameterValue, message exactly "This cluster version doesn't support Register Namespace operation. Please upgrade to the latest cluster version." (the cluster id above is this run's value and is not part of AWS's message).

Cause 3 — redshift.parameter-group-known-parameter-value-validation: 'assert message == "parameter value is not a valid boolean:maybe"' failed with "Invalid value for Parameters.Parameter.1.ParameterValue: 'maybe' is not a value parameter 'require_ssl' allows. Allowed values: true,false". 'maybe' and 'require_ssl' come from the request; the fixed text is 'parameter value is not a valid boolean:'.

Cause 4 — redshift.data-api-statement-id-and-target-validation-rejected: 'assert message == "Redshift endpoint doesn't exist in this region."' failed with 'There should be only one of the following: ClusterIdentifier, WorkgroupName, Glue resource, SessionId' for execute_statement(ClusterIdentifier='<absent-cluster>', WorkgroupName='<absent-workgroup>', ...).

Cause 5 — redshift.data-api-wait-time-seconds-returns-a-terminal-status-without-a-poll: "DATAPLANE_UNIMPLEMENTED: SQL 'SELECT count(*) FROM sim2real_behav_wait_filler a, sim2real_behav_wait_filler b WHERE a.n + b.n = -1' did not FINISH: status='FAILED' error='interrupted'".

Cause 6 — redshift.rdapi-cancelled-statement-aborts-and-second-cancel-rejected: "AssertionError: the long INSERT must still be running when the cancel lands, got 'FAILED' -- it reached a terminal state first".

Hosts, cluster identifiers, table names and the hex labels above are this run's environment; the requirements are the published host being one the emulator serves, the error codes and the message templates quoted.

## Steps to Reproduce
1. CreateCluster, poll DescribeClusters to ClusterStatus 'available', and read Cluster.Endpoint.Address/Port: the address is <cluster>.<hex>.<region>.redshift.amazonaws.com, a host the emulator does not serve (cause 1).
2. CreateCluster and, while ClusterStatus is still 'creating', call RegisterNamespace(NamespaceIdentifier={ProvisionedIdentifier:{ClusterIdentifier:<id>}}, ConsumerIdentifiers=[<account>]): observe InvalidClusterState instead of InvalidParameterValue with AWS's Register-Namespace message (cause 2).
3. CreateClusterParameterGroup(family redshift-2.0) then ModifyClusterParameterGroup(Parameters=[{ParameterName:'require_ssl', ParameterValue:'maybe'}]): observe the wire-location message instead of 'parameter value is not a valid boolean:maybe' (cause 3).
4. redshift-data ExecuteStatement with both ClusterIdentifier=<absent cluster> and WorkgroupName=<absent workgroup>: observe the mutual-exclusivity message instead of "Redshift endpoint doesn't exist in this region." (cause 4).
5. Through the Data API, run a self-join SELECT count(*) over a seeded table with an always-false predicate and poll DescribeStatement: it settles FAILED/'interrupted' (cause 5).
6. Seed a 1,000-row table, submit an INSERT built from its three-way self-join, and read DescribeStatement immediately: it is already FAILED, so a concurrent CancelStatement never applies (cause 6).
7. Re-replay the checks listed in trace_ids.
