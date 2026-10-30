Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# CreateCluster's reused-clientToken ConflictException is hand-built and reports a resourceType with no resourceId

_Filed: 2026-10-28T20:58:10Z_
_Operation: CreateCluster | Class: error-response_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCluster` is now the ONLY handler in this crate that builds a modelled identity-carrying error inline instead of through `crate::state::ResourceIdentity`. Its `clientToken` mismatch refusal constructs `ConflictException` by hand with `resource_type: Some("cluster")` and `resource_id: None`, so the 409 body names a resource TYPE with no instance behind it — a half-filled identity pair, where the crate's own rule is that the two members describe ONE resource, type and own identifier together.

Every one of the six sibling token-mismatch refusals (UpdateCluster, DeleteCluster, PutClusterPolicy, DeleteClusterPolicy, CreateStream, DeleteStream) goes through `ResourceIdentity::...client_token_mismatch()` and reports the resource the refused call addressed, and the very next refusal in THIS handler — the per-account cluster ceiling, twenty lines below — reports the resource that holds the collection through `ResourceIdentity::account()`, with a comment stating that the identity pair is built "through the one helper every refusal of this crate reports an identity with". The token-mismatch arm above it skips that helper, and the crate already carries the identity for the resource a refusal about the per-account collection names (`ResourceIdentity::account()`, documented as "the resource a refusal about a per-account collection reports").

This is NOT the already-fixed cluster-ceiling defect (`20261028T144958Z-dsql-createcluster-error-response`), which was about the `ServiceQuotaExceededException` pairing "cluster" with the account id and which that fix routed through `ResourceIdentity::account()`; the `ConflictException` arm beside it was left hand-built, and is what this issue is about.

## Evidence

The hand-built refusal — `services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:326-336`:
```rust
                crate::state::TokenVerdict::Mismatch => {
                    // No cluster of this request exists to name: the token belongs
                    // to another request, and the identifier this one would have
                    // minted was never minted.
                    return Err(CreateClusterErrorModeled::ConflictException(ConflictException {
                        message: crate::state::CLIENT_TOKEN_MISMATCH_MESSAGE.to_string(),
                        resource_id: None,
                        resource_type: Some(crate::state::RESOURCE_TYPE_CLUSTER.to_string()),
                    })
                    .into());
                }
```

The rule stated in the SAME handler, for the refusal immediately below it — `src/ops/create_cluster.rs:346-351`:
```rust
        // The resource whose quota was exceeded is the one that HOLDS the
        // collection at its ceiling — here the account that owns the clusters, not
        // a cluster, and least of all the cluster this refused call never minted —
        // so the identity pair is built from that resource through the one helper
        // every refusal of this crate reports an identity with
        // (`crate::state::ResourceIdentity`).
```
and that refusal doing it — `src/ops/create_cluster.rs:352-356`: `crate::state::ResourceIdentity::account().quota_exceeded(` …

The helper this arm bypasses — `src/state.rs:1689-1707`:
```rust
    /// The modelled `ConflictException` for a conflict about THIS resource, worded
    /// with `message`.
    ///
    /// The one place a conflict's identity pair is filled, so every refusal that
    /// names a resource names one resource's own type and own identifier.
    pub fn conflict(&self, message: String) -> ConflictException {
…
    /// The modelled `ConflictException` a call whose `clientToken` is already
    /// recorded against a DIFFERENT request answers with, reported against this
    /// resource — the one the refused call addressed.
    pub fn client_token_mismatch(&self) -> ConflictException {
```

The rule the crate states for the pair — `src/state.rs:1626-1638`: "How one resource is identified in the modelled shapes that carry a resource's identity (`ResourceNotFoundException` and `ServiceQuotaExceededException` … and `ConflictException`, which models both as optional): the type segment the resource is stored under, and that resource's own identifier. / The two members describe ONE resource … so a refusal never pairs one resource's type with another's identifier. / Built from the store rather than per handler … so every operation that reports a resource's identity reports the same two values for the same resource."

The identity the crate already has for this case — `src/state.rs:1657-1671`: "The identity of the account this emulator serves … The account is what HOLDS the clusters of this service, so it is the resource a refusal about a per-account collection reports".

The six siblings that all call the helper: `src/ops/update_cluster.rs:286-287`, `src/ops/delete_cluster.rs:227-228`, `src/ops/put_cluster_policy.rs:231-232`, `src/ops/delete_cluster_policy.rs:211-212`, `src/ops/create_stream.rs:292-293`, `src/ops/delete_stream.rs:223-224` — each `crate::state::ResourceIdentity::<kind>(…).client_token_mismatch()`. A grep of the whole `src/ops` tree for an inline `ConflictException {` literal matches only `create_cluster.rs:330`.

Test coverage: `tests/all/error_contract.rs:453-469` (`a_create_cluster_client_token_names_the_request_it_first_served`) asserts only `conflict.status == 409`, `conflict.error == Some("ConflictException")` and the message, never the identity pair; and `tests/all/refusal_resource_identity.rs:113-202` pins the identity of `CreateStream`'s token conflict — "the refusal must report the resource's own type and identifier" — with no `CreateCluster` row, so this body is unpinned.

## Steps to Reproduce

1. Start the emulator and POST /cluster with {"deletionProtectionEnabled": false, "clientToken": "tok-1"} -> 200.
2. POST /cluster again with the SAME clientToken "tok-1" but a different parameter, e.g. {"deletionProtectionEnabled": true, "clientToken": "tok-1"} -> 409 ConflictException (as tests/all/error_contract.rs:446-457 drives it).
3. Read the 409 body: `resourceType` is "cluster" while `resourceId` is absent entirely — a type with no instance, although the handler's own comment says no cluster of this request exists to name.
4. Compare with the sibling refusal of the same class: repeat the token reuse against CreateStream (tests/all/refusal_resource_identity.rs:118-201) and observe both members filled with the addressed cluster's type and identifier.
5. Compare with the OTHER refusal of this same handler: fill the store to state::MAX_CLUSTERS and POST /cluster once more — the 402 reports ("account", <12-digit account id>) through ResourceIdentity::account().
