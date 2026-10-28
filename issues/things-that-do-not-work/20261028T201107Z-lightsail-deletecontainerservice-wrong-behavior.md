Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# Deleting a distribution's origin resource leaves the distribution pointing at a resource the account no longer holds — the origin check only DeleteBucket performs

_Filed: 2026-10-28T20:11:07Z_
_Operation: DeleteContainerService | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A distribution's origin "can be a Lightsail instance, bucket, container service, or load balancer", and the crate stores the origin as the name, Region and resource type that `resolve_origin` resolved at create time. Both write paths refuse an origin that resolves to nothing: `CreateDistribution` answers "The origin does not exist: ... is not an instance, bucket, container service or load balancer of this account" and `UpdateDistribution` re-resolves the same way.

The store has one owner of the reverse question — `distributions_with_origin(region, name, resource_type)`, "How many of this account's distributions pull their content from the resource `name` names" — and exactly one of the four deletes that can remove an origin resource calls it: `DeleteBucket`, which turns it into the documented force-delete condition. `DeleteContainerService`, `DeleteInstance` and `DeleteLoadBalancer` never ask it and never rewrite the distribution, so the delete succeeds and the store is left holding a distribution whose `origin` names a resource that does not exist — a configuration this crate's own create and update paths refuse to store, and which `GetDistributions`/`GetDistribution` then report as if it were serving content.

Each of those three deletes also states that nothing of the removed resource survives it: `DeleteContainerService` says "no later read reports any of them, the published public URL stops answering", `remove_load_balancer` says "no read can observe a certificate or a registration of a load balancer this account no longer holds", and `release_attachments_to_instance` claims to "Release every reference the store holds to the instance" while enumerating only four kinds of record — none of them a distribution.

## Evidence

The store's one owner of the origin-reference question — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:13743-13764:
```
    /// How many of this account's distributions pull their content from the
    /// resource `name` names in `region`, which this store resolved to
    /// `resource_type`.
...
    pub fn distributions_with_origin(&self, region: &str, name: &str, resource_type: &str) -> usize {
```
Its ONLY caller is the bucket delete — src/ops/delete_bucket.rs:197-201:
```
                origin_of_distributions: store.distributions_with_origin(
                    &region,
                    &input.bucket_name,
                    crate::state::RESOURCE_TYPE_BUCKET,
                ),
```
(grep for `distributions_with_origin` across the crate returns src/state.rs:13755 and src/ops/delete_bucket.rs:197 only.)

The write paths refuse an origin that resolves to nothing — src/ops/create_distribution.rs:322-329:
```
            let resolved = store
                .resolve_origin(&origin_region, &origin_name)
                .ok_or_else(|| {
                    not_found(format!(
                        "The origin does not exist: {origin_name} is not an instance, bucket, \
                         container service or load balancer of this account in {origin_region}"
```
and the resolver covers instance, bucket, container service and load balancer — src/state.rs:13712-13738.

The deletes that remove those resources never look — src/ops/delete_container_service.rs:160-174:
```
            // "Deletes your Amazon Lightsail container service": the whole
            // record goes, and with it everything that lives on it — ... so no later read reports any of them,
            // the published public URL stops answering, and the name is free
...
            store
                .remove_container_service(&region, &input.service_name)
                .ok_or_else(|| not_found(&input.service_name))?;
```
src/state.rs:14148-14167 (`remove_load_balancer`: "so no read can observe a certificate or a registration of a load balancer this account no longer holds" — a `remove` of one map entry) and src/state.rs:13498-13592 (`release_attachments_to_instance`: "Release every reference the store holds to the instance `name`", walking disks, static IPs, load balancers and buckets only).

Test coverage: tests/all/bucket_distribution_origin_delete.rs pins the bucket half of this rule (the force-delete condition); tests/all/container_service_delete.rs, tests/all/load_balancer_delete.rs and tests/all/instance_delete_attachments.rs never create a distribution over the resource they delete, so no test pins the dangling origin.

## Steps to Reproduce

1. CreateContainerService with serviceName "origin-svc", power "nano", scale 1 — answered 200.
2. CreateDistribution with distributionName "dist-1", origin {"name":"origin-svc","regionName":"us-east-1","protocolPolicy":"http-only"}, a bundleId of GetDistributionBundles and the default cache behavior (signed for us-east-1) — answered 200; the stored origin is name "origin-svc", resourceType "ContainerService".
3. DeleteContainerService with serviceName "origin-svc" — answered 204, with no condition checked and no distribution rewritten.
4. GetDistributions: "dist-1" still reports origin {name: "origin-svc", resourceType: "ContainerService"}, while GetContainerServices reports no such service.
5. CreateDistribution with a second distributionName and the same origin name: refused with "The origin does not exist: origin-svc is not an instance, bucket, container service or load balancer of this account in us-east-1" — the state the store now holds is one the create refuses to produce.
6. The same sequence with an instance origin + DeleteInstance, or a load-balancer origin + DeleteLoadBalancer, leaves the same dangling origin; only the bucket origin is guarded (DeleteBucket refuses without forceDelete, naming "it is the origin of N distributions").
