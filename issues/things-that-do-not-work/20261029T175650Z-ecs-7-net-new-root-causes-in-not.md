Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.cluster-foreign-account-arn-rejected ecs.tag-nonexistent-resource-fails ecs.stop-task-nonexistent-fails ecs.cluster-lifecycle-active-to-deleted ecs.tag-resource-then-list ecs.op-contract-listClusters ecs.op-contract-listDaemons ecs.container-instance-ops-on-missing-cluster-not-found ecs.service-ops-on-missing-cluster-not-found ecs.invalid-parameter-task-definition-ops
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.cluster-foreign-account-arn-rejected|run#10|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: ecs.tag-nonexistent-resource-fails|run#8|AssertionError|assert code == "InvalidParameterException", f"TagResource cross-account ARN: got {code}"
Issue-Key: ecs.stop-task-nonexistent-fails|run#7|AssertionError|assert message == "AccountIDs mismatch", f"unexpected error message: {message!r}"
Issue-Key: ecs.cluster-lifecycle-active-to-deleted|run#16|AssertionError|assert code == "ClientException", (
Issue-Key: ecs.tag-resource-then-list|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ecs.op-contract-listClusters|run#14|AssertionError|assert err == "InvalidParameterException", f"expected InvalidParameterException, got {err}"
Issue-Key: ecs|ListDaemons|ClusterNotFoundException
Issue-Key: ecs.container-instance-ops-on-missing-cluster-not-found|run#14|AssertionError|assert code == "ClientException", f"RegisterContainerInstance: got {code}"
Issue-Key: ecs.service-ops-on-missing-cluster-not-found|run#37|AssertionError|assert message == "", (
Issue-Key: ecs.invalid-parameter-task-definition-ops|run#14|AssertionError|assert error == expected, f"DescribeTaskDefinition: expected {expected!r}, got {error!r}"
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# ecs: 7 net-new root causes in not-found / error-classification semantics — cross-account ARNs, INACTIVE clusters, pagination tokens, ListDaemons, RegisterContainerInstance, DeleteService and DescribeTaskDefinition

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCluster

## Description
Seven independent ECS error-classification defects. Each cause below must be closed before this issue is submitted.

Cause 1 of 7: an ARN whose account segment is not the caller's is resolved by name instead of refused.
AWS: ECS validates an ARN's account segment before it resolves the resource. DeleteCluster with a well-formed cluster ARN in another account is refused InvalidParameterException with the fixed message "AccountIDs mismatch"; TagResource and ListTagsForResource with such an ARN are refused InvalidParameterException "The ARN provided is invalid."; StopTask with a task ARN in another account is refused InvalidParameterException "AccountIDs mismatch". (Smithy: all four operations model InvalidParameterException, httpError 400, beside ClusterNotFoundException/ResourceNotFoundException; the message text is recorded behavior.) The fixed text is the whole sentence in each case — nothing in these messages comes from the request.
Emulator: the account segment is ignored and the identifier is resolved as a bare name, so DeleteCluster and TagResource/ListTagsForResource answer ClusterNotFoundException, and StopTask answers the message 'The referenced task was not found'.
Why: a client cannot distinguish "this ARN is not yours" from "this resource does not exist" — the first is a credentials/programming error it must stop on, the second is retried or treated as already-deleted. Cross-account guards written against the real contract never trigger here.
Fix: a shared ARN parse-and-authorize step in state.rs (or the request-ARN helper) applied before resource resolution in src/ops/delete_cluster.rs, tag_resource.rs, list_tags_for_resource.rs and stop_task.rs.
Traces: ecs.cluster-foreign-account-arn-rejected, ecs.tag-nonexistent-resource-fails, ecs.stop-task-nonexistent-fails.

Cause 2 of 7: a deleted cluster is reported as nonexistent by one path and as fully live by another, never as INACTIVE.
AWS: DeleteCluster tombstones the cluster as INACTIVE, and the tombstone is what subsequent calls see. UpdateClusterSettings against it is refused ClientException whose message is "Cluster <clusterName> is not in ACTIVE state" (fixed text with the cluster name from the request). ListTagsForResource against the same deleted cluster ARN is refused InvalidParameterException "The specified cluster is inactive. Specify an active cluster and try again." (Smithy: UpdateClusterSettings models ClientException and ClusterNotFoundException; ListTagsForResource models InvalidParameterException.)
Emulator: UpdateClusterSettings on the deleted cluster answers ClusterNotFoundException — the not-found code, not the not-ACTIVE one — while ListTagsForResource on the very same ARN answers HTTP 200 with the tags still attached, as if the cluster were live. The two paths disagree with each other and both disagree with AWS.
Why: a client that reads tags to decide whether a cluster still exists concludes it does; a client that retries a settings update on a not-found cluster recreates it instead of surfacing the inactive-state error. The INACTIVE state is simply not observable.
Fix: the INACTIVE-cluster guard in src/ops/update_cluster_settings.rs and src/ops/list_tags_for_resource.rs, reading the tombstone DeleteCluster writes in state.rs.
Traces: ecs.cluster-lifecycle-active-to-deleted, ecs.tag-resource-then-list.

Cause 3 of 7: a nextToken ECS never minted is refused with the wrong error class.
AWS: ListClusters with an unparseable nextToken is refused InvalidParameterException with the message "Invalid token <token>" — fixed text "Invalid token " followed by the token value from the request. (Smithy: ListClusters models InvalidParameterException and ClientException, both httpError 400.)
Emulator: the same call is refused with code ClientException.
Why: paginating clients branch on InvalidParameterException to abandon a stale cursor and restart; ClientException is the generic class they treat as a request defect, so a resumable walk becomes a hard failure.
Fix: the token-decode refusal in src/ops/list_clusters.rs and the shared cursor decoder it calls.
Traces: ecs.op-contract-listClusters.

Cause 4 of 7: ListDaemons refuses a well-formed cluster ARN that names no cluster.
AWS: ListDaemons does not behave like most cluster-scoped reads — a syntactically valid, same-account, same-region cluster ARN that does not exist produces an empty `daemonSummariesList` and no nextToken, not a not-found error. (Recorded behavior; Smithy: ListDaemons does model ClusterNotFoundException, but a structurally invalid cluster identifier is instead refused ServerException "Service Unavailable. Please try again later.")
Emulator: ListDaemons with a well-formed-but-nonexistent cluster ARN raises ClusterNotFoundException "Cluster not found."
Why: a client enumerating daemons across a set of cluster ARNs (some since deleted) gets an exception instead of an empty page and cannot complete the sweep.
Fix: the cluster-resolution branch of src/ops/list_daemons.rs — return an empty list for an unresolved but well-formed ARN.
Traces: ecs.op-contract-listDaemons.

Cause 5 of 7: RegisterContainerInstance against a missing cluster uses the wrong error class.
AWS: RegisterContainerInstance reports its two failure modes with different classes — against a REAL cluster a missing/invalid identity document is InvalidParameterException "The identity document and identity document signature were not valid.", while against a cluster that does not exist it is ClientException with the message "Cluster not found." (Smithy: RegisterContainerInstance models both ClientException and InvalidParameterException, httpError 400.) The sibling container-instance operations report a missing cluster as ClusterNotFoundException "Cluster not found."
Emulator: RegisterContainerInstance against the nonexistent cluster answers InvalidParameterException — the same class it uses for the real-cluster case — so the request-shape error and the missing-cluster error are indistinguishable.
Why: an agent-registration client retries an identity-document error but must not retry a missing cluster; collapsing the two makes the outcomes look identical.
Fix: the ordering and classification of the cluster lookup versus identity-document validation in src/ops/register_container_instance.rs.
Traces: ecs.container-instance-ops-on-missing-cluster-not-found.

Cause 6 of 7: DeleteService's ServiceNotFoundException carries a message AWS leaves empty.
AWS: against a real cluster and a service name that does not exist, UpdateService is refused ServiceNotFoundException with the message "Service not found." while DeleteService is refused ServiceNotFoundException with an EMPTY message — the two operations differ, and the empty message is the recorded contract for DeleteService. (Smithy: both model ServiceNotFoundException, httpError 400.)
Emulator: DeleteService answers ServiceNotFoundException with the message 'Service not found.', reusing the UpdateService text.
Why: the message is part of the error contract; a client asserting the recorded DeleteService shape sees text where AWS sends none.
Fix: the not-found refusal in src/ops/delete_service.rs.
Traces: ecs.service-ops-on-missing-cluster-not-found.

Cause 7 of 7: DescribeTaskDefinition's malformed-identifier refusal uses emulator wording.
AWS: DescribeTaskDefinition with a malformed task-definition identifier is refused ClientException with the message "Family contains invalid characters." — fixed text, no interpolation. (Smithy: DescribeTaskDefinition models ClientException and InvalidParameterException, httpError 400.)
Emulator: the code is right but the message is 'taskDefinition must be a family, family:revision or task-definition ARN; got "<the supplied identifier>"' — emulator-internal prose that echoes the request value AWS never echoes.
Why: a client matching the recorded sentence does not recognise the refusal, and the echoed request value leaks caller input into an error AWS keeps fixed.
Fix: the identifier-validation refusal text in src/ops/describe_task_definition.rs and the shared task-definition identifier parser it calls.
Related already-fixed issue 20261022T152302Z-ecs-error-messages-interpolate-request-values-instead-of.md addressed this class for other operations; DescribeTaskDefinition still interpolates.
Traces: ecs.invalid-parameter-task-definition-ops.

## Evidence
Cause 1 — DeleteCluster(cluster="arn:aws:ecs:<region>:111122223333:cluster/does-not-exist-xyz"): expected InvalidParameterException "AccountIDs mismatch", got ClusterNotFoundException. TagResource(resourceArn=<own cluster ARN with the account segment swapped>): expected InvalidParameterException "The ARN provided is invalid.", got ClusterNotFoundException. StopTask(task="arn:aws:ecs:<region>:111122223333:task/<cluster>/000...0"): expected InvalidParameterException message "AccountIDs mismatch", got 'The referenced task was not found'.
Cause 2 — UpdateClusterSettings on a cluster just deleted: expected ClientException "Cluster <name> is not in ACTIVE state", got 'ClusterNotFoundException'. ListTagsForResource on that same deleted cluster ARN: expected InvalidParameterException "The specified cluster is inactive. Specify an active cluster and try again.", got HTTP 200 {'tags': [{'key': 'team', 'value': 'platform'}]}.
Cause 3 — ListClusters(nextToken="sim2real-totally-bogus-token"): expected InvalidParameterException (message "Invalid token <that token>"), got ClientException.
Cause 4 — ListDaemons(clusterArn=<well-formed ARN for a cluster that does not exist>): expected {"daemonSummariesList": [], no nextToken}, got ClusterNotFoundException "Cluster not found."
Cause 5 — RegisterContainerInstance(cluster="no-such-cluster-..."): expected ClientException "Cluster not found.", got InvalidParameterException.
Cause 6 — DeleteService(cluster=<real cluster>, service="nonexistent-svc"): expected ServiceNotFoundException with Message "", got Message 'Service not found.'
Cause 7 — DescribeTaskDefinition(taskDefinition="not a valid @@@ identifier"): expected ('ClientException', 'Family contains invalid characters.'), got ('ClientException', 'taskDefinition must be a family, family:revision or task-definition ARN; got "not a valid @@@ identifier"').
Generated cluster/service/task names and the 000000000000 account in the recording are per-run values, not requirements.

## Steps to Reproduce
Cause 1: CreateCluster, then DeleteCluster with a cluster ARN identical in shape but with account 111122223333; TagResource and ListTagsForResource with your own cluster ARN with its account segment replaced; StopTask with a task ARN in another account on a real cluster. Read each code and message.
Cause 2: CreateCluster, DeleteCluster (it reports INACTIVE), then UpdateClusterSettings(cluster=<arn>, settings=[{containerInsights, enabled}]) and ListTagsForResource(resourceArn=<arn>).
Cause 3: ListClusters(nextToken="sim2real-totally-bogus-token").
Cause 4: CreateCluster, ListDaemons(clusterArn=<that ARN>) (empty), then ListDaemons with the same ARN prefix and a cluster name that does not exist.
Cause 5: CreateCluster <real>; RegisterContainerInstance(cluster=<real>) (InvalidParameterException, identity-document text) and RegisterContainerInstance(cluster="no-such-cluster-<rand>").
Cause 6: CreateCluster + CreateService <svc>; UpdateService(cluster=<real>, service="nonexistent-svc") then DeleteService(cluster=<real>, service="nonexistent-svc"); compare the two messages.
Cause 7: DescribeTaskDefinition(taskDefinition="not a valid @@@ identifier").
Replay the check ids in trace_ids to confirm.
