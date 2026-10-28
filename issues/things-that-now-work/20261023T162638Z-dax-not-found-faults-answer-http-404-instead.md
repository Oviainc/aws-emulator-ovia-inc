Issue-Source: trace-sweep-divergence
Issue-Trace: dax-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dax-error-paths
Issue-Shape-Id: com.amazonaws.dax#AmazonDAXV3
Issue-Key: dax|DescribeClusters|4xx->4xx|$.message
Issue-Key: dax|DescribeClusters|4xx->4xx|status
Issue-Key: dax|DeleteCluster|4xx->4xx|status
Issue-Key: dax|UpdateCluster|4xx->4xx|status
Issue-Key: dax|IncreaseReplicationFactor|4xx->4xx|status
Issue-Key: dax|DeleteParameterGroup|4xx->4xx|status
Issue-Key: dax|DescribeSubnetGroups|4xx->4xx|status
Issue-Commit: c5ab1277cf41f7c1d08660bb7be869a7fe462875

# DAX not-found faults answer HTTP 404 instead of 400 on every resolve-by-name path

> _Note: this issue was identified against the code base at commit `c5ab1277cf41f7c1d08660bb7be869a7fe462875`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
This bundle carries 6 causes, all one shared fault: DAX is an awsJson1_1 service (smithy_query service_info: protocols=["awsJson1_1"], sdkId DAX, version 2017-04-19), and every recorded DAX client fault is served with HTTP 400. The model's @httpError on the not-found faults is 404 and is marked inert for this protocol (smithy_query shape: ClusterNotFoundFault httpError 404 / awsQueryError {code ClusterNotFound, inert: true}; ParameterGroupNotFoundFault 404 inert; SubnetGroupNotFoundFault 404 inert). The emulator is evidently applying that inert @httpError as the response status, so every operation that resolves a resource by name answers 404 where AWS answers 400. The error code/__type itself matches in all six steps -- only the HTTP status (and in cause 1 also the message rule) diverges.

Cause 1 of 6: DescribeClusters on a nonexistent cluster (fixture dax-error-paths, step task#1).
  AWS: ClusterNotFoundFault, HTTP 400 (recorded response status 400; Smithy: DescribeClusters errors include ClusterNotFoundFault, whose @httpError 404 is inert under awsJson1_1). The recorded message names the cluster that was asked for -- 'Cluster <name> not found.' -- so the message is built from the request and that derivation is itself the requirement; the recorded text is an example of it.
  Emulator: HTTP 404, and a generic message that does not name the requested cluster (state.rs's cluster_not_found() returns a fixed 'Cluster not found.').
  Why: an SDK classifies a 404 differently from a 400 -- clients that branch on the HTTP status (and tooling that treats 4xx subclasses distinctly) take the wrong path, and a caller that parses the message to learn which of several requested cluster names was missing cannot.
  Fix: the dax error serialization path that maps a modeled fault to an HTTP status (it must use the awsJson1_1 400 for client faults, not the model's inert @httpError), plus the fault message builder in state.rs for ClusterNotFoundFault.
  Traces: dax-error-paths#task#1. Code: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs (cluster_not_found(), ~line 1006).

Cause 2 of 6: DeleteCluster on a nonexistent cluster (fixture dax-error-paths, step task#2).
  AWS: ClusterNotFoundFault with HTTP 400 (recorded status 400; Smithy: DeleteCluster errors include ClusterNotFoundFault).
  Emulator: HTTP 404.
  Why: same status misclassification on the delete path; a client retry/branch policy keyed on status sees a resource-level 404 instead of a validation-class 400.
  Fix: the shared dax fault-to-status mapping (same component as cause 1).
  Traces: dax-error-paths#task#2.

Cause 3 of 6: UpdateCluster on a nonexistent cluster (fixture dax-error-paths, step task#3).
  AWS: ClusterNotFoundFault with HTTP 400 (recorded status 400; Smithy: UpdateCluster errors include ClusterNotFoundFault).
  Emulator: HTTP 404.
  Why: as cause 2, on the modify path.
  Fix: the shared dax fault-to-status mapping.
  Traces: dax-error-paths#task#3.

Cause 4 of 6: IncreaseReplicationFactor on a nonexistent cluster (fixture dax-error-paths, step task#5).
  AWS: ClusterNotFoundFault with HTTP 400 (recorded status 400; Smithy: IncreaseReplicationFactor errors include ClusterNotFoundFault).
  Emulator: HTTP 404.
  Why: as cause 2, on the scale-out path.
  Fix: the shared dax fault-to-status mapping.
  Traces: dax-error-paths#task#5.

Cause 5 of 6: DeleteParameterGroup on a nonexistent parameter group (fixture dax-error-paths, step task#7).
  AWS: ParameterGroupNotFoundFault with HTTP 400 (recorded status 400; Smithy: DeleteParameterGroup errors include ParameterGroupNotFoundFault, @httpError 404 inert).
  Emulator: HTTP 404.
  Why: as cause 2, for parameter groups.
  Fix: the shared dax fault-to-status mapping.
  Traces: dax-error-paths#task#7.

Cause 6 of 6: DescribeSubnetGroups naming a nonexistent subnet group (fixture dax-error-paths, step task#10).
  AWS: SubnetGroupNotFoundFault with HTTP 400 (recorded status 400; Smithy: DescribeSubnetGroups errors are ServiceLinkedRoleNotFoundFault and SubnetGroupNotFoundFault, the latter @httpError 404 inert).
  Emulator: HTTP 404.
  Why: as cause 2, for subnet groups.
  Fix: the shared dax fault-to-status mapping.
  Traces: dax-error-paths#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):
- dax-error-paths#task#1 DescribeClusters: status expected 400, got 404; $.message expected 'Cluster sim2real-probe-missing-cluster-23bbaef0 not found.' (example of the request-derived rule), got 'Cluster not found.'
- dax-error-paths#task#2 DeleteCluster: status expected 400, got 404
- dax-error-paths#task#3 UpdateCluster: status expected 400, got 404
- dax-error-paths#task#5 IncreaseReplicationFactor: status expected 400, got 404
- dax-error-paths#task#7 DeleteParameterGroup: status expected 400, got 404
- dax-error-paths#task#10 DescribeSubnetGroups: status expected 400, got 404
In each case the error code/__type matched; only the HTTP status (and cause 1's message rule) diverged. Model evidence: smithy_query(service_info) -> protocols [awsJson1_1]; smithy_query(shape) -> ClusterNotFoundFault/ParameterGroupNotFoundFault/SubnetGroupNotFoundFault all carry httpError 404 with awsQueryError marked inert.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture dax-error-paths.
2. Step task#1: DescribeClusters with ClusterNames naming a cluster that does not exist -> emulator returns HTTP 404 with message 'Cluster not found.'; AWS returns HTTP 400 with ClusterNotFoundFault whose message names the requested cluster.
3. Steps task#2 / task#3 / task#5: DeleteCluster, UpdateCluster, IncreaseReplicationFactor against the same nonexistent ClusterName -> HTTP 404 instead of 400.
4. Step task#7: DeleteParameterGroup with a ParameterGroupName that does not exist -> HTTP 404 instead of 400.
5. Step task#10: DescribeSubnetGroups with SubnetGroupNames naming a group that does not exist -> HTTP 404 instead of 400.
