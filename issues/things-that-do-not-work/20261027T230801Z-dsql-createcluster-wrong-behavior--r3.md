Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# fingerprint_tags renders a tag map ambiguously, so a clientToken reused with DIFFERENT tags replays instead of conflicting

_Filed: 2026-10-27T23:08:01Z_
_Operation: CreateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `clientToken` ledger decides "retry" vs "conflict" by comparing a hash of the request's parameters (`crate::state::request_fingerprint`). The tag part of that hash is built by `crate::state::fingerprint_tags`, which renders each entry as `{key}={value}` and joins the entries with `,`. `TagKey` and `TagValue` both admit the `=` character, so two DIFFERENT tag maps render to the SAME part: `{"a": "b=c"}` and `{"a=b": "c"}` both render `a=b=c`.

Consequence on CreateCluster: a second `CreateCluster` carrying the same `clientToken` but a different tag map hashes identically, so the ledger answers `TokenVerdict::Replay` and the handler returns the FIRST call's cluster with HTTP 200. The cluster the caller asked for (with the different tags) is never created and the `ConflictException` the operation models for a reused token — the answer the handler's own doc promises — is never returned: an error is silently turned into a success. The same helper backs `CreateStream`'s fingerprint (`create_stream.rs:262`), so a `CreateStream` retry with a different tag map is likewise mistaken for a retry and mints no stream.

The crate already states the rule this helper breaks one level up: `request_fingerprint` length-prefixes each part precisely so that two different parameter lists cannot hash alike, but the rendering INSIDE the tag part has no such separation.

## Evidence

The rule the crate states for itself, `services/service-com-amazonaws-dsql-dsql/src/state.rs:613-621`:
```
pub fn request_fingerprint(parts: &[String]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // The length prefix keeps the parts unambiguous: without it
        // ["a", "bc"] and ["ab", "c"] hash alike.
        hasher.update((part.len() as u64).to_be_bytes());
```
The tag rendering that has no such protection, `src/state.rs:624-638`:
```
/// Render a tag map into one fingerprint part, in key order.
pub fn fingerprint_tags(tags: Option<&std::collections::HashMap<String, String>>) -> String {
    ...
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
```
The model admits `=` in both halves of an entry — `smithy_query(query="shape", shape="TagKey,TagValue")`: `TagKey` `"pattern": "^[a-zA-Z0-9_.:/=+\\-@ ]*$"`, `TagValue` the same — so `{"a": "b=c"}` and `{"a=b": "c"}` are both valid and both render `a=b=c`.

The handler doc the code then breaks, `src/ops/create_cluster.rs:14-15`: "the same token with different parameters is refused as a conflict", and `src/ops/create_cluster.rs:290-293`:
```
// `@idempotencyToken`: a repeat of the SAME request returns the result of
// the original call and has no additional effect; the same token with
// DIFFERENT parameters is a different request wearing a used token, which
// is a conflict rather than a retry.
```
The call site that feeds the ambiguous part in, `src/ops/create_cluster.rs:417-425`:
```
fn create_fingerprint(input: &CreateClusterInput) -> String {
    crate::state::request_fingerprint(&[
        ...
        crate::state::fingerprint_tags(input.tags.as_ref()),
```
and the same helper on the stream path, `src/ops/create_stream.rs:257-263`.

Test coverage: `tests/all/idempotent_replay_result.rs`, `tests/all/create_cluster_replay_configuration.rs` and `tests/all/delete_cluster_idempotency.rs` exercise replay/mismatch, but no test varies a tag map whose key or value contains `=`, so nothing pins the ambiguous rendering.

## Steps to Reproduce

1. POST /cluster with body {"deletionProtectionEnabled": false, "clientToken": "tok-1", "tags": {"a": "b=c"}} -> 200, note the returned identifier C1.
2. POST /cluster again with body {"deletionProtectionEnabled": false, "clientToken": "tok-1", "tags": {"a=b": "c"}} — a different request (different tag map) wearing the used token.
3. Observe HTTP 200 whose identifier/arn are C1's: the ledger treated it as a retry because `fingerprint_tags` rendered both maps as `a=b=c`.
4. GET /cluster -> only C1 exists; no cluster carrying the tag key `a=b` was created, and no 409 ConflictException was returned.
5. Repeat with CreateStream (POST /stream/{clusterIdentifier}, same clientToken, tags {"a": "b=c"} then {"a=b": "c"}) for the same silent replay.
