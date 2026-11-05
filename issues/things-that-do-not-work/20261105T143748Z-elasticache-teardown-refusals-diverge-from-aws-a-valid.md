Issue-Source: trace-sweep-divergence
Issue-Trace: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution
Issue-Step: teardown#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution elasticache-vvd-traj-cache-subnet-group-build-mistakes elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes elasticache-vvd-traj-replication-group-id-and-topology-mistakes elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes elasticache-cache-parameter-group-workflow
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|DeleteCacheSubnetGroup|2xx->4xx|status
Issue-Key: elasticache|DescribeCacheParameterGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: ef7f816f5820fcb580018a7c921bd985d34fbc86

# ElastiCache teardown refusals diverge from AWS: a valid DeleteCacheSubnetGroup answers 400 and the CacheParameterGroup not-found message text is wrong

> _Note: this issue was identified against the code base at commit `ef7f816f5820fcb580018a7c921bd985d34fbc86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCacheSubnetGroup

## Description
Two net-new wire divergences replayed from the simparity-wire-aws-v1 corpus against the elasticache emulator. Both are refusal/teardown paths of the elasticache control plane; neither is marked ALREADY FILED or OWNED by an open issue in the replay report. There are 2 causes and both must be closed.

Cause 1 of 2: DeleteCacheSubnetGroup refuses the delete of a cache subnet group the same fixture created, answering HTTP 400 where AWS answers HTTP 200.
AWS: Smithy: DeleteCacheSubnetGroup input CacheSubnetGroupName (String, required); output shape Unit; SUCCESS status 200; modeled errors CacheSubnetGroupInUse (HTTP 400, awsQuery code CacheSubnetGroupInUse) and CacheSubnetGroupNotFoundFault (HTTP 400, awsQuery code CacheSubnetGroupNotFoundFault). Operation doc: "Deletes a cache subnet group. You cannot delete a default cache subnet group or one that is associated with any clusters." AWS therefore answers 200 with an empty DeleteCacheSubnetGroupResult envelope when the named, non-default group exists and no cache cluster or replication group is associated with it - which is exactly the state the recorded teardown reaches, since the recording shows status 200 for this step in all five fixtures.
Emulator: answers HTTP 400 for that same delete. The divergence is reported at the status level ("status: expected 200, got 400"), so the body is one of the operation's modeled 400 refusals rather than the modeled success.
Hypothesis: the group was never admitted into the store, so the delete resolves nothing and the handler takes its CacheSubnetGroupNotFoundFault branch - the already-open issues 20261102T083323Z-elasticache-cache-subnet-group-creation-and-engine-version.md and 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md record CreateCacheSubnetGroup refusing valid subnet ids with InvalidParameterValue "Some input subnets in :[...] are invalid.". Those issues do NOT carry this step's replay key (the replay report lists this cluster with an empty owned_by), so this delete must be re-replayed in its own right: if that create refusal is the shared cause, fixing it must make these five teardown steps answer 200, and if it is not, the handler's own not-found / in-use gating is wrong. Whatever the mechanism, the requirement is that a delete of an existing, unassociated, non-default cache subnet group answers 200 with an empty result and removes the group, so a following DescribeCacheSubnetGroups for the name reports it absent.
Why: a client that creates a cache subnet group and then deletes it cannot complete teardown. Every delete is a 400, so cleanup scripts, CloudFormation stack deletes and test harnesses abort or leak the resource, and the name stays occupied against the account's subnet-group quota. Because the refusal is a modeled client error, the client has no way to tell "you may not delete this" from "the emulator never stored it".
Fix: the DeleteCacheSubnetGroup handler in the elasticache service crate - its resolve-then-gate sequence (the not-found branch and the in-use branch over cache_clusters / replication_groups) and the CreateCacheSubnetGroup state write it depends on.
Traces: elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#teardown#10 (representative), plus elasticache-vvd-traj-cache-subnet-group-build-mistakes#teardown#12, elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#teardown#14, elasticache-vvd-traj-replication-group-id-and-topology-mistakes#teardown#10, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#teardown#10 - one cluster, 5 affected steps.

Cause 2 of 2: the CacheParameterGroupNotFound message from DescribeCacheParameterGroups is worded as a colon-prefixed debug string instead of AWS's sentence template.
AWS: Smithy: DescribeCacheParameterGroups models CacheParameterGroupNotFoundFault with awsQuery code CacheParameterGroupNotFound and HTTP 404. The recorded response for a name that does not exist carries status 404, that code, and the message "CacheParameterGroup <name> not found." - the fixed text is "CacheParameterGroup " followed by " not found.", and the only variable part is the cache parameter group NAME, taken verbatim from the request's CacheParameterGroupName member. The status (404) and the error code matched; only the message text diverged.
Emulator: answers the same 404 and the same error code but the message "CacheParameterGroup not found: <name>" - the predicate and the name are transposed, the name is emitted after a colon instead of inside the sentence, and the terminating period is lost.
Why: the error code alone does not identify which parameter group is missing when a request or a workflow names several, so clients and operators parse the name out of the message and tests assert on its exact text. A transposed, colon-delimited message defeats both byte-exact assertions and name extraction, and the inconsistency with the rest of the service's refusals makes the emulator unusable as a wording reference. The already-open issue 20261102T083323Z-elasticache-refusals-carry-the-wrong-error-code-and.md covers other elasticache refusal wording but does not carry this step's replay key, so this path is net-new.
Fix: the CacheParameterGroupNotFoundFault message template used by the DescribeCacheParameterGroups handler's lookup path (and the shared state helper that formats it, so every operation resolving a cache parameter group by name words it identically).
Traces: elasticache-cache-parameter-group-workflow#teardown#15.

## Evidence
Replayed corpus simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, 227 steps, 147 passed, score 64.8%, 38 divergence clusters). Both clusters below come back with stub_path="" and owned_by=[] - i.e. not blocked by an unimplemented operation and not owned by any open issue.

Cause 1 - fixture elasticache-tgwc-traj-cache-subnet-group-vpc-resolution, step teardown#10, service elasticache, operation DeleteCacheSubnetGroup, verdict mismatch:
  divergence_key: service=elasticache|DeleteCacheSubnetGroup|status=200->400|keys=status
  expected_status: 200
  actual_status:   400
  detail: "status: expected 200, got 400"
  affected_count: 5 - elasticache-tgwc-traj-cache-subnet-group-vpc-resolution#teardown#10, elasticache-vvd-traj-cache-subnet-group-build-mistakes#teardown#12, elasticache-vvd-traj-replication-group-auth-and-network-reference-mistakes#teardown#14, elasticache-vvd-traj-replication-group-id-and-topology-mistakes#teardown#10, elasticache-vvd-traj-valkey-engine-version-and-node-type-mistakes#teardown#10
  Expected per the Smithy model: 200 with the empty DeleteCacheSubnetGroupResult envelope (output shape Unit). Got: a modeled 400 client refusal (CacheSubnetGroupNotFoundFault or CacheSubnetGroupInUse).

Cause 2 - fixture elasticache-cache-parameter-group-workflow, step teardown#15, service elasticache, operation DescribeCacheParameterGroups, verdict mismatch:
  divergence_key: service=elasticache|DescribeCacheParameterGroups|status=404->404|keys=$.ErrorResponse.Error.Message
  expected_status: 404, actual_status: 404 (status and error code agree)
  detail: "$.ErrorResponse.Error.Message: value_mismatch (expected 'CacheParameterGroup sim2real-probe-elasticache-cpg-9547f4d1 not found.', got 'CacheParameterGroup not found: sim2real-probe-elasticache-cpg-9547f4d1')"
  The generated name sim2real-probe-elasticache-cpg-9547f4d1 is a per-run value and is NOT part of the requirement; the requirement is the template "CacheParameterGroup <CacheParameterGroupName> not found." with the name taken from the request.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus against the emulator built from the checkout. Both clusters appear with stub_path="" and owned_by=[].

2. Cause 1 - DeleteCacheSubnetGroup must answer 200:
   a. Create a VPC with at least two subnets in different AZs (ec2 CreateVpc / CreateSubnet).
   b. elasticache CreateCacheSubnetGroup(CacheSubnetGroupName='<name>', CacheSubnetGroupDescription='<d>', SubnetIds=[<s1>, <s2>]).
   c. With no cache cluster and no replication group referring to '<name>', call elasticache DeleteCacheSubnetGroup(CacheSubnetGroupName='<name>').
   Expected: HTTP 200 with an empty DeleteCacheSubnetGroupResult, and a following DescribeCacheSubnetGroups(CacheSubnetGroupName='<name>') reporting the group absent. Observed: HTTP 400 with a modeled client refusal. Re-replay all five fixture steps listed under Evidence, not just the representative - the cluster covers teardown steps in five different fixtures.

3. Cause 2 - CacheParameterGroupNotFound message template:
   a. elasticache DescribeCacheParameterGroups(CacheParameterGroupName='<a-name-that-does-not-exist>').
   Expected: HTTP 404, error code CacheParameterGroupNotFound, message exactly "CacheParameterGroup <that-name> not found." (fixed text "CacheParameterGroup " ... " not found.", name substituted from the request). Observed: HTTP 404 with the correct code but the message "CacheParameterGroup not found: <that-name>".
   b. Re-replay elasticache-cache-parameter-group-workflow#teardown#15 to confirm.
