Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# A retried CreateNodeRegistrationScript hands out the script for a cluster whose delete has been accepted, the refusal the same handler makes for a fresh token

_Filed: 2026-10-29T12:17:41Z_
_Operation: CreateNodeRegistrationScript | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateNodeRegistrationScript` resolves its `RequestId` and returns the already-issued script BEFORE the cluster is resolved. The cluster resolution one call below goes through `live_cluster`, which rules out a cluster whose delete has been accepted — its own comment says "a cluster whose delete has been accepted is leaving the account, so there is no endpoint for a new node of it to register against" — and the handler answers that with the modelled `BadRequestException` naming the cluster as not found.

The token reader `node_registration_script_for_token` re-checks only the cluster ID RECORDED ON THE SCRIPT (`node_registration_script` filters `record.cluster_id == cluster_id`), never whether that cluster is still one the account may register nodes in, and nothing forgets a script or its token when a cluster is deleted. So the same request is answered two opposite ways depending only on whether its `RequestId` was spent before: a fresh token is refused 400, while the spent token is answered 200 with a registration script for an endpoint the crate itself says does not exist — a script whose `CreateNode` step the service would then refuse.

## Evidence

The replay branch, which answers before the cluster is resolved — services/service-com-amazonaws-medialive-medialive/src/ops/create_node_registration_script.rs:186-197:
```
        // A retried request (the same `@idempotencyToken` `RequestId`, presented
        // to the same cluster's endpoint) must answer with the script that one
        // issued: a second script would register a second node per hardware unit.
        if let Some(issued) = input
            .request_id
            .as_deref()
            .and_then(|token| state.node_registration_script_for_token(&input.cluster_id, token))
        {
            return Ok(CreateNodeRegistrationScriptOutput {
                node_registration_script: Some(issued.to_string()),
            });
        }
```

The liveness rule the non-replay path applies, in the very next call — src/state/node_registration_scripts.rs:173-178:
```
        // Resolved through the LIVE cluster, like every other write addressed to
        // one: a cluster whose delete has been accepted is leaving the account,
        // so there is no endpoint for a new node of it to register against.
        if self.live_cluster(cluster_id).is_none() {
            return Err(NodeRegistrationScriptRefusal::ClusterNotFound);
        }
```
and the handler's refusal for it — src/ops/create_node_registration_script.rs:251-264:
```
    let message = match refusal {
        Refusal::ClusterNotFound => crate::state::not_found_message("Cluster", cluster_id),
        ...
    CreateNodeRegistrationScriptErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
    })
```

The token reader, whose only re-check is the cluster the script RECORDS — src/state/node_registration_scripts.rs:255-270:
```
    /// The script a stored `RequestId` issued for `cluster_id`, if the ring still
    /// holds it.
    ///
    /// The cluster is part of the key AND re-checked on the record, exactly as
    /// the node slice re-checks it: a token presented to one cluster's endpoint
    /// says nothing about another's.
    pub fn node_registration_script_for_token(
        &self,
        cluster_id: &str,
        request_id: &str,
    ) -> Option<&str> {
        let id = self
            .node_registration_script_tokens
            .created(&node_registration_script_token_key(cluster_id, request_id))?;
        self.node_registration_script(cluster_id, id)
    }
```
over src/state/node_registration_scripts.rs:232-237, which filters nothing but the cluster id:
```
    pub fn node_registration_script(&self, cluster_id: &str, id: &str) -> Option<&str> {
        self.node_registration_scripts
            .get(id)
            .filter(|record| record.cluster_id == cluster_id)
            .map(|record| record.script.as_str())
    }
```

The crate's own rule for a retry aimed at a parent whose delete has been accepted, stated and pinned for the sibling create — tests/all/input_idempotency_after_delete.rs:334-341: "a `RequestId` cannot turn a request this operation refuses into one it serves, so a retry aimed at an input whose delete has been accepted is refused exactly as a first attempt is" — and the service-wide rule, src/state.rs:82-84: "once a delete has been accepted for an id, that id names no resource a caller may still change."

Test coverage: the retry suite enumerates what the SECOND request carries and never a deleted cluster — tests/all/node_registration_script.rs:432-462 (`a_retried_request_answers_with_the_script_it_issued`, rows "the same token and body", "the same token, another body", "another token", "no token at all", "the same token on another cluster"). The cluster-delete token rule that IS pinned is a different one (the CreateCluster token's own window, tests/all/cluster_deletion.rs:755-777).

## Steps to Reproduce

1. POST /prod/clusters with {"name":"c1","clusterType":"ON_PREMISES"} -> note the cluster id C.
2. POST /prod/clusters/C/nodeRegistrationScript with {"requestId":"R","name":"anywhere-$hn"} -> 200 with a nodeRegistrationScript.
3. DELETE /prod/clusters/C -> 202 with state DELETING (optionally wait for the clock so GET /prod/clusters/C reports the terminal DELETED; the record is kept either way).
4. POST /prod/clusters/C/nodeRegistrationScript with a FRESH requestId -> 400 BadRequestException "Cluster C not found" (src/state/node_registration_scripts.rs:173-178, src/ops/create_node_registration_script.rs:251-264).
5. POST /prod/clusters/C/nodeRegistrationScript again with {"requestId":"R","name":"anywhere-$hn"} -> the emulator answers 200 with the script it issued for the now-deleted cluster. Expected, per the same handler's own refusal and src/state.rs:82-84, the same 400 the fresh-token call gets: the token must not turn a request this operation refuses into one it serves.
