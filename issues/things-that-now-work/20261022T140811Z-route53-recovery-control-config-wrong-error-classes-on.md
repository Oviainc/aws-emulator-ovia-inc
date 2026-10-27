Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.create-cluster-duplicate-name-accepted route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected route53-recovery-control-config.create-control-panel-nonexistent-cluster-not-found route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner route53-recovery-control-config.delete-cluster-with-dependents-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|CreateCluster|ConflictException
Issue-Key: route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: route53-recovery-control-config.create-control-panel-nonexistent-cluster-not-found|run#12|AssertionError|assert code == "ValidationException", code
Issue-Key: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner|run#14|AssertionError|assert all(code == "ValidationException" for _, code in failures), failures
Issue-Key: route53-recovery-control-config.delete-cluster-with-dependents-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: 1e44d2b2032d56c599b46458071bcd07c3617563

# route53-recovery-control-config: wrong error classes on CreateCluster/CreateControlPanel/CreateRoutingControl/DeleteCluster rejection paths, plus a cluster-name uniqueness rule AWS does not have

> _Note: this issue was identified against the code base at commit `1e44d2b2032d56c599b46458071bcd07c3617563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Five distinct root causes in the route53-recovery-control-config emulator, all on rejection/uniqueness paths of the cluster/control-panel/routing-control create and delete operations. Each is grounded in a failing behavioral check. They share one service and one general theme (the service picks error classes from the model's generic error list instead of the class real ARC returns, and invents a uniqueness rule), but each needs its own fix and must be closed separately.

=== Cause 1 of 5: CreateCluster enforces ClusterName uniqueness that AWS does not ===
AWS: recorded behavior only. CreateCluster does NOT enforce ClusterName uniqueness within an account: a second CreateCluster with the exact same ClusterName as a live cluster SUCCEEDS and yields a second, independent cluster (distinct ClusterArn, separately describable, independently taggable, both listed under the shared name). Smithy: CreateCluster models ConflictException (HTTP 409) but nothing in the model requires name uniqueness; ClusterName is only constrained as __stringMin1Max64PatternS (@length 1..64, @pattern ^\S+$).
Emulator: the second CreateCluster is rejected with ConflictException (HTTP 409), message e.g. 'a cluster named "sim2real-r53arc-dupcluster-..." already exists in this account'.
Why: a client that legitimately creates same-named clusters (per-region/per-stack naming, re-created stacks, blue/green) is refused outright; the whole duplicate-name workflow, including the independent-identity guarantees (distinct ARNs, per-cluster tags), is unreachable on the emulator.
Fix: remove the account-wide name-uniqueness check from the CreateCluster handler's state validation so creation proceeds and two records with the same name coexist.
Traces: route53-recovery-control-config.create-cluster-duplicate-name-accepted; emulator rule at services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_cluster.rs lines 222-235.

=== Cause 2 of 5: CreateCluster name-pattern violation answers ValidationException instead of BadRequestException ===
AWS: recorded behavior only. A ClusterName containing whitespace (pattern ^\S+$ violated) is rejected with error code BadRequestException at HTTP 400, with a request-body validation message built from the request, of the form 'Invalid request body : [ECMA 262 regex "..." does not match input string "<the supplied name>"]' (recorded example: regex ^((?![;'\s<>&"])[\u0021-\u007E])+$ against the supplied name). No cluster is created. Smithy: CreateCluster.ClusterName -> __stringMin1Max64PatternS, @length 1..64, @pattern ^\S+$; the modeled error list for CreateCluster is AccessDenied/Conflict/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Throttling/ValidationException — BadRequestException is NOT modeled, so this is a front-door request-body validation class the model does not describe.
Emulator: rejects with ValidationException (the generic modeled constraint-violation class) instead of BadRequestException.
Why: clients and SDK error handling that branch on BadRequestException (botocore exposes it as a distinct exception class) never match, so pattern-violation handling and negative tests behave differently than against AWS.
Fix: the service's input-constraint validation layer for route53-recovery-control-config must map pattern/length violations on the request body to a BadRequestException response (400) carrying the regex-vs-input message built from the request, rather than the generic ValidationException.
Traces: route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected (assertion: expected BadRequestException, got 'ValidationException').

=== Cause 3 of 5: CreateControlPanel with a nonexistent ClusterArn answers ResourceNotFoundException instead of ValidationException ===
AWS: recorded behavior only. CreateControlPanel naming a ClusterArn that does not exist is rejected with ValidationException (HTTP 400) — the create path validates the referenced cluster differently from the read path (DescribeCluster answers ResourceNotFoundException for the same nonexistent cluster). The message is derived from the request and names the cluster, recorded example: 'Cluster <bogus ClusterArn> does not exist.' No panel is created. Smithy: CreateControlPanel models both ResourceNotFoundException (404) and ValidationException (400), so the model alone does not choose; the recording does.
Emulator: rejects with ResourceNotFoundException (HTTP 404), message e.g. 'no cluster is stored under <arn>'.
Why: a client distinguishing a malformed/unknown parent reference (retry never helps, 400) from a missing resource (404) takes the wrong branch; code written against AWS that catches ValidationException on the create path sees an uncaught ResourceNotFoundException.
Fix: in the CreateControlPanel handler's parent-cluster lookup, return the ValidationException class for an unknown ClusterArn instead of ResourceNotFoundException.
Traces: route53-recovery-control-config.create-control-panel-nonexistent-cluster-not-found (assertion: expected ValidationException, got ResourceNotFoundException); emulator branch at src/ops/create_control_panel.rs lines 209-216.

=== Cause 4 of 5: duplicate RoutingControlName in one control panel answers ConflictException instead of ValidationException ===
AWS: recorded behavior only. When concurrent CreateRoutingControl calls race for the identical RoutingControlName in the same control panel, exactly one succeeds and every loser is rejected with ValidationException — empirically NOT the ConflictException the model's generic 409 wording would suggest. Smithy: CreateRoutingControl models both ConflictException (409) and ValidationException (400); the recording selects ValidationException for the duplicate-name refusal.
Emulator: every loser is rejected with ConflictException (HTTP 409) — the check observed four ConflictExceptions where four ValidationExceptions were expected.
Why: a client that treats ConflictException as a transient, retryable collision will retry a request AWS classifies as a permanent input error (and vice versa), so duplicate-name handling diverges.
Fix: in the CreateRoutingControl handler's within-panel name-uniqueness check, emit the ValidationException class instead of ConflictException (the single-winner serialization itself is correct).
Traces: route53-recovery-control-config.concurrent-duplicate-routing-control-name-single-winner; emulator branch at src/ops/create_routing_control.rs lines 247-262.

=== Cause 5 of 5: DeleteCluster with dependents answers ConflictException instead of ValidationException ===
AWS: recorded behavior only. DeleteCluster against a DEPLOYED cluster that still owns a non-default control panel (and a routing control) is rejected with ValidationException (HTTP 400), recorded message 'Cannot delete a Cluster with non-default Control Panels.'; the cluster, panel and routing control all survive, and deleting the dependents first makes the cluster delete succeed. Confirmed against a fully DEPLOYED tree, so it is not a PENDING-state artifact. Smithy: DeleteCluster models ConflictException (409), ResourceNotFoundException (404) and ValidationException (400); the recording selects ValidationException.
Emulator: rejects with ConflictException (HTTP 409), messages e.g. 'cluster <arn> still has 1 control panel(s)' / '... still has N routing control(s)'.
Why: teardown logic written against AWS catches ValidationException to drive the delete-children-first loop; against the emulator it sees ConflictException and either aborts teardown or retries forever, stranding clusters against the account cluster quota.
Fix: in the DeleteCluster handler's dependents checks, emit the ValidationException class instead of ConflictException for both the control-panel and routing-control dependents refusals.
Traces: route53-recovery-control-config.delete-cluster-with-dependents-rejected (assertion: expected ValidationException, got 'ConflictException'); emulator branches at src/ops/delete_cluster.rs lines 170-200.

Note: no existing open issue covers these; the only neighbouring open issue is for route53resolver ('Route 53 Resolver endpoint ops skip resource validation and omit paginator echo'), a different service. The ListClusters / ListControlPanels / UpdateCluster scaffold-stub failures in the same suite run are already filed and are deliberately NOT part of this issue.

## Evidence
From the behavioral suite (route53-recovery-control-config 0/21, these 5 checks):

1. create-cluster-duplicate-name-accepted [error]
   expected: second CreateCluster with the same ClusterName succeeds with a distinct ClusterArn
   actual:   botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the CreateCluster operation: a cluster named "sim2real-r53arc-dupcluster-ee36e1fcbc83" already exists in this account

2. create-cluster-invalid-name-pattern-rejected [mismatch]
   expected: error code BadRequestException (HTTP 400), message 'Invalid request body : [ECMA 262 regex "..." does not match input string "<name>"]'
   actual:   AssertionError: expected BadRequestException, got 'ValidationException'

3. create-control-panel-nonexistent-cluster-not-found [mismatch]
   expected: error code ValidationException (HTTP 400), message 'Cluster <arn> does not exist.'
   actual:   AssertionError: ResourceNotFoundException

4. concurrent-duplicate-routing-control-name-single-winner [mismatch]
   expected: 1 success + 4 failures all with code ValidationException
   actual:   AssertionError: [('error', 'ConflictException'), ('error', 'ConflictException'), ('error', 'ConflictException'), ('error', 'ConflictException')]

5. delete-cluster-with-dependents-rejected [mismatch]
   expected: error code ValidationException (HTTP 400), message 'Cannot delete a Cluster with non-default Control Panels.'
   actual:   AssertionError: expected ValidationException, got 'ConflictException'

Emulator source confirming the chosen classes:
- src/ops/create_cluster.rs:222-235 — account-wide ClusterName uniqueness -> ConflictException
- src/ops/create_control_panel.rs:209-216 — unknown parent cluster -> ResourceNotFoundException
- src/ops/create_routing_control.rs:247-262 — duplicate RoutingControlName in panel -> ConflictException
- src/ops/delete_cluster.rs:170-200 — non-default panels / routing controls present -> ConflictException

## Steps to Reproduce
All against the emulator's route53-recovery-control-config endpoint (boto3 client 'route53-recovery-control-config').

Cause 1: CreateCluster(ClusterName=N); CreateCluster(ClusterName=N) again -> emulator raises ConflictException; AWS returns a second cluster with a distinct ClusterArn (both listable under N, independently describable and taggable).

Cause 2: CreateCluster(ClusterName='has a space') -> emulator raises ValidationException; AWS raises BadRequestException (400) with the 'Invalid request body : [ECMA 262 regex ... does not match input string "has a space"]' body-validation message, and creates nothing.

Cause 3: CreateControlPanel(ClusterArn='arn:aws:route53-recovery-control::<acct>:cluster/00000000-0000-0000-0000-000000000000', ControlPanelName=P) -> emulator raises ResourceNotFoundException; AWS raises ValidationException with 'Cluster <arn> does not exist.'

Cause 4: create a cluster and a control panel, then fire 5 concurrent CreateRoutingControl calls with the SAME RoutingControlName on that panel -> emulator: 1 success + 4 ConflictException; AWS: 1 success + 4 ValidationException (and exactly one routing control of that name listed).

Cause 5: create a cluster, wait for DEPLOYED, create a non-default control panel and a routing control, wait for DEPLOYED, then DeleteCluster(ClusterArn=...) -> emulator raises ConflictException; AWS raises ValidationException 'Cannot delete a Cluster with non-default Control Panels.' with all three resources intact; deleting routing control then panel then cluster succeeds.
