Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 01d4c565076fad2daa34afee2942eef023c65029

# The session-credentials idempotency key is keyed on the RESOLVED cluster/endpoint ids while the replay comparison is made on the raw identifier spelling, so the two identifier forms the handler accepts as equivalent are refused as "different parameters"

_Filed: 2026-10-27T08:05:00Z_
_Operation: GetManagedEndpointSessionCredentials | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `01d4c565076fad2daa34afee2942eef023c65029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`virtualClusterIdentifier` and `endpointIdentifier` may each be given as the resource's bare `ResourceIdString` id or as its full ARN; `identified_virtual_cluster` / `identified_managed_endpoint` resolve BOTH forms to the same stored record, by design. The handler then builds the idempotency scope from the RESOLVED ids, so both spellings share one `(region, scope, clientToken)` key and a replay finds the session the first call issued.

But the replay test is `existing.request_fingerprint != fingerprint`, and the fingerprint is taken from the RAW input (`request_fingerprint(&input)` at line 207), where the identifiers are still the strings the request spelled. The result is that a retry which names the same endpoint through the other accepted form — every other parameter identical, same `clientToken` — is not answered with the first session: it is refused with a 400 `ValidationException` saying the token "was already used ... with different parameters", even though no parameter names anything different. The crate normalises the identifier for the key and does not normalise it for the comparison, so the two halves of its own replay rule disagree.

## Evidence

The key is built from resolved ids — services/.../src/ops/get_managed_endpoint_session_credentials.rs:214-228 resolves `virtual_cluster_id` / `endpoint_id` out of the store, then line 244:
```
        let scope = crate::state::endpoint_session_scope(&virtual_cluster_id, &endpoint_id);
```
with services/.../src/state.rs:342-349:
```
/// The idempotency scope `GetManagedEndpointSessionCredentials` remembers a `clientToken`
/// under: scoped to the managed endpoint as well as the operation, because a session is
/// issued FOR one endpoint of one virtual cluster
/// ... and the token identifies one such request.
pub fn endpoint_session_scope(virtual_cluster_id: &str, endpoint_id: &str) -> String {
```
Both spellings are deliberately equivalent — state.rs:2122-2127:
```
/// The managed endpoint an `endpointIdentifier` names under this virtual cluster, on
/// the terms [`Self::identified_virtual_cluster`] states: its `ResourceIdString` id, or
/// its full `EndpointArn`.
```
and state.rs:2097-2099: "every other operation of this service names a cluster by its `ResourceIdString` id, so BOTH forms resolve here."
The comparison, however, is on the unresolved request — get_managed_endpoint_session_credentials.rs:207 and 252-257:
```
        let fingerprint = crate::state::request_fingerprint(&input);
...
                if existing.request_fingerprint != fingerprint {
                    return Err(invalid_request(format!(
                        "clientToken {client_token} was already used to issue session credentials {}, with different parameters",
```
Test coverage: `tests/all/managed_endpoint_session_credentials.rs:505-561` (RULE 5) replays only with the SAME identifier spelling, and lines 193-207 prove the ARN spelling is an accepted, equivalent way to name the same pair — so nothing pins today's refusal.

## Steps to Reproduce

1. Create a virtual cluster and a managed endpoint, and wait for the endpoint to reach ACTIVE.
2. POST /virtualclusters/{clusterId}/endpoints/{endpointId}/credentials with {"executionRoleArn":"arn:aws:iam::000000000000:role/EMRStudioRole","credentialType":"TOKEN","clientToken":"tok-1"} — note the returned session id and token.
3. Replay the same call with the same clientToken "tok-1" and the same body, but address the path with the URL-encoded virtual-cluster ARN and endpoint ARN instead of the bare ids (the form state.rs:2122-2127 documents as naming the same pair).
4. Expected: the replay answers the session the first call issued (the idempotency key is the same, being built from the resolved ids).
5. Actual: HTTP 400 ValidationException "clientToken tok-1 was already used to issue session credentials <id>, with different parameters" — the fingerprint differs only in the identifier spelling.
