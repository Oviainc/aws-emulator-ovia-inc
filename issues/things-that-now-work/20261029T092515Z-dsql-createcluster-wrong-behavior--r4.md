Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: feb204971e0c529b59922c31c571e9b68abdb6f7

# A CreateCluster clientToken replays across Regions, so a create at one endpoint answers with a cluster minted for another Region

_Filed: 2026-10-29T09:25:15Z_
_Operation: CreateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `feb204971e0c529b59922c31c571e9b68abdb6f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCluster` is the one operation of this crate whose result is derived from the REQUEST's Region: the identifier's ARN, the connection endpoint, the `encryptionDetails.kmsKeyArn` and the witness-Region check are all built from `crate::state::request_region(ctx)`. The `clientToken` fingerprint, however, renders only the request's body members and never that Region, so two creates signed for DIFFERENT Regions with the same token and the same body hash identically and the second is served as a REPLAY of the first.

The caller that asked the eu-west-1 endpoint for a cluster is therefore answered 200 with the identifier, ARN and endpoint of a cluster minted in us-east-1, and no cluster of its own Region is created. The answer names a resource that very endpoint refuses to serve anywhere else in the crate: the tagging resolution (`ArnResource::is_scoped_to`) answers 404 for exactly such an ARN, which the crate's own test pins both directions. `UpdateCluster`, `DeleteCluster`, `CreateStream`, `DeleteStream` and both policy writers are unaffected — each binds its token to the identifier of an existing resource, whose Region is read off the stored ARN (`crate::state::resource_region`) rather than off the request — so `CreateCluster` is the only fingerprint that drops a value its own answer is derived from.

## Evidence

The Region the handler HONOURS, and the fingerprint that omits it — `src/ops/create_cluster.rs:228`:
```rust
        let region = crate::state::request_region(ctx);
```
used for the identity it mints at `src/ops/create_cluster.rs:278-279`:
```rust
        let identifier = state.mint_cluster_identifier();
        let arn = crate::state::cluster_arn(&region, &identifier);
```
and for the endpoint and encryption at `src/ops/create_cluster.rs:371-374`:
```rust
            encryption_details: Some(crate::state::encryption_details(&region, customer_key)),
            ...
            endpoint: Some(crate::state::cluster_endpoint(&region, &identifier)),
```
against `src/ops/create_cluster.rs:421-433`:
```rust
/// The parameters a `CreateCluster` `clientToken` is bound to: every member of
/// the request except the token itself, so a retry carrying the same token is
/// recognised only when it asks for the same cluster.
fn create_fingerprint(input: &CreateClusterInput) -> String {
    crate::state::request_fingerprint(&[
        input.deletion_protection_enabled.to_string(),
        crate::state::fingerprint_optional(input.kms_encryption_key.as_deref()),
        crate::state::fingerprint_multi_region(input.multi_region_properties.as_ref()),
        crate::state::fingerprint_optional(input.policy.as_deref()),
        input.bypass_policy_lockout_safety_check.to_string(),
        crate::state::fingerprint_tags(input.tags.as_ref()),
    ])
}
```
The doc's claim — "recognised only when it asks for the same cluster" — is what the omission breaks, because this crate states that the Region is part of a cluster's identity — `src/state.rs:1464-1468`:
```rust
/// a resource nested under it, a name or an endpoint that fronts it, the KMS key
/// ARN one of its members expands into — is qualified with this region, not with
/// the region the request happens to be signed for: the region an ARN was minted
/// in is part of the resource's identity ([`ArnResource::is_arn_of`]) rather than a
/// property of the call that reads or changes it. Only the call that MINTS a
/// resource takes the region from the request, which is then the one its ARN
/// carries ever after.
```
and that an ARN of another Region names nothing this endpoint may serve — `src/state.rs:1350-1353`:
```rust
    /// pattern pins all three segments
    /// (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`),
    /// so they are part of the resource's identity rather than decoration. An ARN
    /// differing in any of them therefore names a resource of some OTHER region,
    /// account or partition — one this endpoint holds none of, and must neither read
    /// nor write.
```
The rule a create at an endpoint already owes, pinned over one shared store — `tests/all/tagging_arn_scope.rs:429-458`:
```rust
    // And a cluster the other endpoint creates ITSELF is served there and nowhere
    // else: the same rule, read from the other region's side.
    let created = away
        .create(json!({"deletionProtectionEnabled": false, "tags": initial_tags()}))
        .await;
    ...
    assert_eq!(
        away_arn,
        expected_arn(AWAY, &created.str("identifier")),
        "the cluster must carry its own region's ARN, got {away_arn:?}"
    );
```
That test drives the two-Region/one-store setup with NO `clientToken`, and `tests/all/client_token_parameter_distinctness.rs:156` (`a_create_cluster_token_is_bound_to_the_values_of_its_composite_members`) varies only body members, so no case pairs one token with two signing Regions and the collision is unpinned.

## Steps to Reproduce

1. Build one engine and two endpoint views over it, as `tests/all/tagging_arn_scope.rs` does: `let engine = Arc::new(engine::InMemoryEngine::new()); let home = Dsql::sharing(&engine, "us-east-1"); let away = Dsql::sharing(&engine, "eu-west-1");`
2. POST /cluster at `home` with {"deletionProtectionEnabled": false, "clientToken": "tok-1"} -> 200 with arn `arn:aws:dsql:us-east-1:<account>:cluster/<id>` and endpoint `<id>.dsql.us-east-1.on.aws`.
3. POST /cluster at `away` with the SAME body and the SAME clientToken "tok-1".
4. Expected: a cluster of the eu-west-1 endpoint, carrying `arn:aws:dsql:eu-west-1:<account>:cluster/<new-id>` (the rule tagging_arn_scope.rs:440-444 asserts for a create at that endpoint).
5. Actual: 200 replaying the first call — the us-east-1 identifier, ARN and endpoint — and GET /cluster at `away` lists only that one cluster, so no eu-west-1 cluster exists.
6. GET /tags/{the arn just answered} at `away` -> 404 ResourceNotFoundException: the create's own answer names a resource that endpoint refuses to read or write.
