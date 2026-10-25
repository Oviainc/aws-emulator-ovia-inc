Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d7f2082fceb83802a8fa1cf49a5205bc74f19057

# DeleteBucket skips the "bucket is the origin of a distribution" force-delete condition, on the strength of a comment the crate now contradicts

_Filed: 2026-10-25T00:21:18Z_
_Operation: DeleteBucket | Class: validation_

> _Note: this issue was identified against the code base at commit `d7f2082fceb83802a8fa1cf49a5205bc74f19057`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The modelled `DeleteBucketRequest$forceDelete` documentation lists FOUR conditions a bucket must be force-deleted under. The handler enumerates and checks only three (objects, access keys, resources granted access via SetResourceAccessForBucket) and deliberately omits the fourth — "the bucket is the origin of a distribution" — on the grounds, stated in its own comment, that "a distribution is a resource this emulator does not hold".

That justification is no longer true inside this same crate: the store holds a `distributions` map, `CreateDistribution` is implemented, and `CreateDistribution` resolves a BUCKET as a distribution origin (it even has a branch that refuses `enablePrivateOriginAccess` unless the resolved origin `resource_type` IS a bucket). So a caller can create a bucket, create a distribution whose origin is that bucket, then call `DeleteBucket` with no `forceDelete` and the bucket is removed with a plain 200 — the condition that the model says REQUIRES `forceDelete` never fires, and the distribution is left pointing at an origin that no longer exists.

Note also that `SetResourceAccessForBucket` is still a scaffold stub, so `resources_receiving_access` is always empty; the distribution-origin condition is therefore the one remaining reachable condition the handler never evaluates.

## Evidence

Smithy model (`smithy_query(query="doc", shape="com.amazonaws.lightsail#DeleteBucketRequest", member="forceDelete")`):
> "You must force delete the bucket if it has one of the following conditions:" … "The bucket is the origin of a distribution." … "The bucket has instances that were granted access to it using the SetResourceAccessForBucket action." … "The bucket has objects." … "The bucket has access keys."

Handler, services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_bucket.rs:202-208:
```
// "You must force delete the bucket if it has one of the following conditions: The
// bucket is the origin of a distribution. ... The bucket has objects. The bucket has
// access keys." Three of those four are conditions this store knows about; a
// distribution is a resource this emulator does not hold, so no bucket here is the
// origin of one.
```
and the check that follows only ever inspects three things — delete_bucket.rs:209-227 (`if objects > 0`, `if held.access_keys > 0`, `if held.resources_receiving_access > 0`) — with `HeldBucket` carrying exactly those two stored counts (delete_bucket.rs:331-338).

The same crate DOES hold distributions, and a bucket IS an admissible origin — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_distribution.rs:317-339:
```
// "An origin can be a Lightsail instance, bucket, container
// service, or load balancer" ...
let resolved = store
    .resolve_origin(&origin_region, &origin_name)
...
if private_origin_access && resolved.resource_type != crate::state::RESOURCE_TYPE_BUCKET
```
and create_distribution.rs:349 `if store.distributions.len() >= crate::state::MAX_DISTRIBUTIONS`.

Test coverage: `tests/all/bucket_object_data_plane.rs:631-726` exercises `forceDelete` only for the objects and access-keys conditions (`grep -rn forceDelete tests/` finds no distribution-origin case), and `tests/all/distribution_cdn.rs:147,180` creates a bucket (`ORIGIN_BUCKET`) specifically to be a distribution origin — so nothing in the crate pins the current behaviour.

## Steps to Reproduce

1. POST /ls/api/2016-11-28/CreateBucket with {"bucketName":"origin-bucket","bundleId":"small_1_0"} against the us-east-1 endpoint.
2. POST /ls/api/2016-11-28/CreateDistribution with {"distributionName":"cdn-1","origin":{"name":"origin-bucket","regionName":"us-east-1","protocolPolicy":"http-only"},"defaultCacheBehavior":{"behavior":"cache"},"bundleId":<an active distribution bundle>} — succeeds, and the stored origin reports resourceType Bucket.
3. POST /ls/api/2016-11-28/DeleteBucket with {"bucketName":"origin-bucket"} and NO forceDelete.
4. Observe HTTP 200 and the bucket removed, instead of the InvalidInputException the modelled forceDelete rule requires while the bucket is the origin of a distribution.
5. POST /ls/api/2016-11-28/GetDistributions with {"distributionName":"cdn-1"} — the distribution still reports origin.name 'origin-bucket', which no longer exists.
