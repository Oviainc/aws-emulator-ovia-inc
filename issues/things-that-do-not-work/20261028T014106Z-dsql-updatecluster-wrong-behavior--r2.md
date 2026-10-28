Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c550c8d329f766442e2f60b5ca19de7c84eb66f9

# UpdateCluster rebuilds encryptionDetails.kmsKeyArn in the REQUEST's Region, so a cross-Region update stores a KMS key ARN in a Region the cluster does not live in

_Filed: 2026-10-28T01:41:06Z_
_Operation: UpdateCluster | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c550c8d329f766442e2f60b5ca19de7c84eb66f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateCluster derives the cluster's Region from the cluster's own ARN for the witness-Region check — with an explicit comment that the Region "is a property of the cluster" — and then, a few lines later, expands a supplied `kmsEncryptionKey` into a KMS key ARN using the REQUEST's Region instead of that `cluster_region` it already computed. Cross-Region requests over one store are a supported and tested scenario in this crate, so the two differ: a cluster minted in us-east-1 that is updated through a eu-west-1-signed endpoint ends up persisting `encryptionDetails.kmsKeyArn = arn:aws:kms:eu-west-1:<acct>:key/<id>`, which GetCluster then reports for a cluster whose own ARN is in us-east-1. The value is written into stored state, so every later read of the cluster carries it.

## Evidence

The handler computes the cluster's Region, stating the rule:

`services/service-com-amazonaws-dsql-dsql/src/ops/update_cluster.rs:233-239`
```
// ... The cluster's
// Region is the one its ARN was minted in, not the request's: it is a
// property of the cluster.
let cluster_region = state
    .clusters
    .get(&input.identifier)
    .and_then(|cluster| crate::state::arn_region(&cluster.arn))
    .unwrap_or_else(|| region.clone());
```
and then ignores it when it writes the cluster's encryption record:

`services/service-com-amazonaws-dsql-dsql/src/ops/update_cluster.rs:327-332`
```
if let Some(kms_encryption_key) = input.kms_encryption_key.as_deref() {
    let customer_key = crate::state::customer_managed_key(Some(kms_encryption_key));
    cluster.encryption_details =
        Some(crate::state::encryption_details(&region, customer_key));
    cluster.kms_encryption_key = customer_key.map(str::to_string);
}
```
where `region` is the request's (`update_cluster.rs:216`: `let region = crate::state::request_region(ctx);`) and the expansion interpolates it verbatim:

`services/service-com-amazonaws-dsql-dsql/src/state.rs:1356-1363`
```
fn kms_key_arn(region: &str, kms_encryption_key: &str) -> String {
    if kms_encryption_key.starts_with("arn:") { ... }
    else { ::engine::emulator_arn("kms", region, &format!("key/{kms_encryption_key}")) }
}
```
The sibling that faces the same question uses the cluster's Region, and a test pins that rule for cross-Region requests over one store:

`services/service-com-amazonaws-dsql-dsql/src/ops/get_vpc_endpoint_service_name.rs:183-188` ("the region of the ARN the cluster was minted with, so a request signed for another region that reads this cluster still gets the cluster's own") and `services/service-com-amazonaws-dsql-dsql/tests/all/vpc_endpoint_service_name.rs:162-163`
```
/// RULE: the region in the name is the CLUSTER's (the region of the ARN it was minted
/// with), not the region the reading request happens to be signed for.
```
The encryption tests only ever use one view: `tests/all/encryption_key_selection.rs:137` / `:205` build `Dsql::in_region(region)` and create AND update through it, so the cross-Region case is uncovered.

## Steps to Reproduce

1. Build one `engine::InMemoryEngine` and two views: `Dsql::sharing(&engine, "us-east-1")` and `Dsql::sharing(&engine, "eu-west-1")` (harness.rs:68).
2. CreateCluster through the us-east-1 view; wait for the clock so it is ACTIVE. Its ARN carries `:dsql:us-east-1:`.
3. UpdateCluster that identifier through the eu-west-1 view with `{"kmsEncryptionKey": "my-key"}`; wait for the clock.
4. GetCluster the cluster (from either view) and read `encryptionDetails.kmsKeyArn`: `arn:aws:kms:eu-west-1:<acct>:key/my-key`, a Region the cluster's own ARN does not name, while the handler had already derived the cluster's Region as us-east-1.
