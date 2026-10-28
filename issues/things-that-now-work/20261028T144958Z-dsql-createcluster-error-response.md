Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 15aaa5471fc3f7278352228b97d2216e65bea584

# The cluster-ceiling ServiceQuotaExceededException pairs resourceType "cluster" with the ACCOUNT id as resourceId

_Filed: 2026-10-28T14:49:58Z_
_Operation: CreateCluster | Class: error-response_

> _Note: this issue was identified against the code base at commit `15aaa5471fc3f7278352228b97d2216e65bea584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCluster` is the only operation in this crate that builds a `ServiceQuotaExceededException` by hand instead of through `crate::state::ResourceIdentity::quota_exceeded`, and the identity pair it fills does not describe one resource: it reports `resourceType: "cluster"` beside `resourceId: emulator_account_id()`. A 12-digit account id is not a cluster identifier — `ClusterId` is `^[a-z0-9]{26}$`, so no cluster of this service can ever carry that id — so a client reading the modelled pair is told that a cluster whose id cannot exist exceeded a quota. Every sibling quota refusal in the crate goes through the one helper and reports the holder's OWN type and OWN identifier (`CreateStream` for the per-cluster stream ceiling, `TagResource` for the per-resource tag ceiling), and the crate states that rule for itself in `ResourceIdentity`'s doc. This is the same defect class that was already filed and fixed for `CreateStream` (`20261027T230801Z-dsql-createstream-error-response`), left behind on the one path that never adopted the helper.

## Evidence

The inline construction — `services/service-com-amazonaws-dsql-dsql/src/ops/create_cluster.rs:345-358`:
```
        if state.clusters.len() >= crate::state::MAX_CLUSTERS {
            return Err(CreateClusterErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "The maximum number of clusters ({}) has already been created.",
                        crate::state::MAX_CLUSTERS
                    ),
                    resource_id: ::engine::emulator_account_id().to_string(),
                    resource_type: crate::state::RESOURCE_TYPE_CLUSTER.to_string(),
```

The rule the crate states for itself — `src/state.rs:1614-1622` (`ResourceIdentity`): "How one resource is identified in the modelled shapes that carry a resource's identity (`ResourceNotFoundException` and `ServiceQuotaExceededException`, both of which model a `@required` `resourceId` and `resourceType`...): the type segment the resource is stored under, and that resource's own identifier. / The two members describe ONE resource — the model words them as the resource that "could not be found" and the resource that "exceeds a quota" — so a refusal never pairs one resource's type with another's identifier."

The one helper it bypasses — `src/state.rs:1681-1694`: "The modelled `ServiceQuotaExceededException` for a collection THIS resource holds being at its ceiling: the resource whose quota was exceeded is the one that holds the collection, so the refusal reports that resource's own type and identifier..." with `resource_id: self.resource_id.clone(), resource_type: self.resource_type.clone(),`.

The siblings that use it — `src/ops/create_stream.rs:319-329`:
```
        if cluster.streams.len() >= crate::state::MAX_STREAMS_PER_CLUSTER {
            return Err(CreateStreamErrorModeled::ServiceQuotaExceededException(
                crate::state::ResourceIdentity::cluster(&cluster.identifier).quota_exceeded(
```
and `src/ops/tag_resource.rs:254-262`: `return Err(TagResourceErrorModeled::ServiceQuotaExceededException(identity.quota_exceeded(`.

The model's own wording of the two members (`smithy_query(query="shape", shape="ServiceQuotaExceededException")`): `resourceId` — "The resource ID exceeds a quota.", `resourceType` — "The resource type exceeds a quota."; and `smithy_query(query="shape", shape="ClusterId")`: `"pattern": "^[a-z0-9]{26}$"`, which an account id does not satisfy.

Test coverage: `tests/all/cluster_lifecycle.rs:909-945` (`the_cluster_store_is_bounded_by_its_ceiling`) asserts only `refused.status == 402` and `refused.error == Some("ServiceQuotaExceededException")` — it never reads `resourceId`/`resourceType`; and the identity table in `tests/all/refusal_resource_identity.rs:307-320` lists only `DeleteStreamTokenReuse`, `CreateStreamOnACreatingCluster`, `DeleteStreamOfACreatingStream`, `GetStreamOfAnAbsentStream`, so the cluster ceiling is the one quota refusal nothing pins.

## Steps to Reproduce

1. Start the emulator and POST /cluster with {"deletionProtectionEnabled": false} `state::MAX_CLUSTERS` (100) times, as tests/all/cluster_lifecycle.rs:912-921 does.
2. POST /cluster once more with {"deletionProtectionEnabled": false}.
3. Observe the 402 body: `resourceType` is "cluster" while `resourceId` is the 12-digit emulator account id.
4. Read the reported pair back the way tests/all/refusal_resource_identity.rs:61-72 does — GET /cluster/<resourceId> — and note the value is not even a `ClusterId` (`^[a-z0-9]{26}$`), so it names no cluster of the reported type.
5. Compare with the per-cluster stream ceiling (create 20 streams on one cluster, then one more): that 402 reports the holder's own type and identifier via `ResourceIdentity::cluster(...).quota_exceeded(...)`.
