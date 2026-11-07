Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected
Issue-Shape-Id: com.amazonaws.neptunegraph#AmazonNeptuneGraph
Issue-Key: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected|<lambda>#9|Timeout|lambda: denied_client.execute_query(
Issue-Commit: 4def4dd98da6b6421456df50766a3afbdc4b34e9

# ExecuteQuery never returns an IAM authorization denial: a principal with no policies is not refused with AccessDeniedException naming the principal and the graph

> _Note: this issue was identified against the code base at commit `4def4dd98da6b6421456df50766a3afbdc4b34e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
One net-new root cause in neptune-graph's data plane, grounded in the failing behavioral check `neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected`.

Cause 1 of 1: ExecuteQuery makes no authorization decision for the calling principal, so an IAM user with no attached policies is never refused with AccessDeniedException. Failing check ids: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected.

AWS: Smithy (services/service-com-amazonaws-neptunegraph-amazonneptunegraph/smithy-model/com-amazonaws-neptunegraph-amazonneptunegraph.json) models ExecuteQuery as POST /queries with hostPrefix `{graphIdentifier}.` and declares `AccessDeniedException` (errorKind client, httpError 403) among its errors. On real AWS, a caller whose credentials belong to an IAM principal with no policy granting the graph's data-plane actions is refused with HTTP 403, error code `AccessDeniedException`, and the standard IAM denial message. That message's FIXED text is `User: ` ... ` is not authorized to perform: ` ... ` on resource: `; its VARIABLE parts are taken from the request's signing principal and the targeted resource — the caller's principal ARN (`arn:aws:iam::<account>:user/<user-name>`), the required action list for an openCypher query (`neptune-graph:DeleteDataViaQuery, neptune-graph:ReadDataViaQuery, neptune-graph:WriteDataViaQuery`), and the graph ARN (`arn:aws:neptune-graph:<region>:<account>:graph/<graph-id>`). Recorded behavior also shows a legitimate transient on a freshly minted access key: the same code `AccessDeniedException` with message `Authentication Failed` while the key propagates; the authorization decision proper is the denial that names the principal and the graph, and the check polls past the propagation phase to observe it.

Emulator: the authorization outcome is never produced. Replaying the check, the no-policy caller's ExecuteQuery call is answered with something other than an `AccessDeniedException` whose message names the principal and the graph for the entire polling window, and the check ends `Check timed out after 180.0s` without ever observing a denial. In this same run every ExecuteQuery call is answered `NotImplemented` (HTTP 501, `operation ExecuteQuery is scaffolded but not implemented in this emulator`), i.e. the request is rejected before any principal/permission evaluation happens and the credential attached to the request has no bearing on the response. Note: the open issue 20261023T175427Z-neptune-graph-executequery-stub.md covers only implementing the ExecuteQuery handler (its Issue-Trace-Ids do not include this check), and 20261104T060310Z-neptune-graph-executequery-answers-notimplemented-501-for-a-malformed-opencypher.md covers only the malformed-query ValidationException path; neither requires the authorization decision this check asserts, so implementing the query engine alone leaves this check failing (it would then SUCCEED for the unauthorized principal, which the check treats as an over-permissive failure).

Why: a client relying on AWS's behavior gets authorization wrong in both directions. Code that tests least-privilege policies, or that distinguishes a denial (fix the policy) from a transient (retry), sees neither: it gets a 501 today and, once the handler lands with no authorization check, an unauthorized principal silently reads and writes graph data. Negative-path tests of IAM policy for neptune-graph data-plane actions cannot pass against the emulator, and an application that surfaces the denial message to operators has no principal/resource/action information to show.

Fix: the ExecuteQuery handler and its authorization step in services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/execute_query.rs — resolve the request's signing principal, evaluate it against the graph resource for the openCypher data actions, and on no grant serialize AccessDeniedException with HTTP 403 and the IAM denial message templated from the request principal and graph ARN; the positive control (a permissioned caller running `RETURN 1 AS one` on the same graph and receiving `{"results": [{"one": 1}]}`) must still succeed.

Traces: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected (verdict mismatch; `Check timed out after 180.0s` while polling for `AccessDeniedException` with a message other than `Authentication Failed`).

## Evidence
Check: neptune-graph.neptune-graph-execute-query-missing-permission-denied-rejected (group `overly-permissive`, tags axis:rejection, concern:data-plane, plane:data).

Expected (real AWS): calling ExecuteQuery(graphIdentifier=<g-id>, queryString="RETURN 1 AS one", language="OPEN_CYPHER") with the access key of a brand-new IAM user that has no attached policies is refused with HTTP 403, Error.Code `AccessDeniedException`, and Error.Message
  "User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: neptune-graph:DeleteDataViaQuery, neptune-graph:ReadDataViaQuery, neptune-graph:WriteDataViaQuery on resource: arn:aws:neptune-graph:<region>:<account>:graph/<graph-id>"
(fixed text as quoted; <account>, <user-name>, <region>, <graph-id> are filled from the request's principal and the targeted graph). A freshly created key may first be refused as `AccessDeniedException` / "Authentication Failed" while it propagates — that is the transient, not the authorization decision. Immediately afterwards, the originally permissioned caller's ExecuteQuery on the SAME graph still succeeds and its payload parses to {"results": [{"one": 1}]}.

Actual (emulator): no `AccessDeniedException` naming the principal and the graph is ever returned; the poll for that denial never satisfies and the check fails with `Check timed out after 180.0s`. Every ExecuteQuery call in this replay is answered HTTP 501 `NotImplemented` — "operation ExecuteQuery is scaffolded but not implemented in this emulator" — so the request's credentials are never evaluated and no authorization verdict (deny or allow) is reachable.

Smithy citation: ExecuteQuery errors include AccessDeniedException { errorKind: client, httpError: 403 }; http { method: POST, uri: /queries }, endpoint hostPrefix "{graphIdentifier}.".

## Steps to Reproduce
1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=<unique>, provisionedMemory=16, deletionProtection=false, publicConnectivity=true) and poll GetGraph until status == AVAILABLE; note the graph id <g-id>.
3. iam.CreateUser(UserName=<unique>) with NO policies attached, then iam.CreateAccessKey for that user.
4. Build a second neptune-graph client using only that user's access key id / secret.
5. Call ExecuteQuery(graphIdentifier=<g-id>, queryString="RETURN 1 AS one", language="OPEN_CYPHER") with the no-policy client, retrying for up to a couple of minutes to let the key propagate.
6. Observe that no HTTP 403 AccessDeniedException whose message names arn:aws:iam::<account>:user/<user-name>, the neptune-graph:*DataViaQuery actions and arn:aws:neptune-graph:<region>:<account>:graph/<g-id> is ever returned (in this build every call is HTTP 501 NotImplemented); the poll runs until the check's 180s budget expires.
7. Positive control: call the same ExecuteQuery with the original, permissioned credentials and observe it must return payload {"results": [{"one": 1}]}.
