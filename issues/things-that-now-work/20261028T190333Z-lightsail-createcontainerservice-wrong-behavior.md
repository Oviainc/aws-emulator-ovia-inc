Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# The container-service mutating calls write no operation-log entry, so GetOperations and GetOperationsForResource never report them

_Filed: 2026-10-28T19:03:33Z_
_Operation: CreateContainerService | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other mutating call in this crate appends an entry to the account's operation log through `store.start_operation(...)` / `store.record_untyped_operation(...)` — 70+ handlers do, and the crate states the rule twice as an invariant of the store. The six container-service mutating handlers write none:

* `CreateContainerService` (src/ops/create_container_service.rs)
* `UpdateContainerService` (src/ops/update_container_service.rs)
* `CreateContainerServiceDeployment` (src/ops/create_container_service_deployment.rs)
* `DeleteContainerService` (src/ops/delete_container_service.rs)
* `RegisterContainerImage` (src/ops/register_container_image.rs)
* `DeleteContainerImage` (src/ops/delete_container_image.rs)

None of those files calls `start_operation`, `start_untyped_operation` or `record_untyped_operation` at all, and none carries a comment admitting or justifying the omission. The consequence is observable through two other implemented operations of this same crate: `GetOperations` ("Reports the whole operation log of the account") and `GetOperationsForResource` never report a single entry for a container service, so a caller that creates, updates, deploys to or deletes a container service, or registers an image to one, sees an empty log for it — while the same caller sees entries for a bucket, an instance, a disk, a distribution, a load balancer, an alarm, a certificate, a contact method or a VPC peering.

The one container call that deliberately logs nothing — `CreateContainerServiceRegistryLogin` — spells out exactly WHEN an entry is not written, and its reasoning does not reach these six: a container service HAS a name and the modelled `ResourceType` enum HAS a `ContainerService` member (the crate's own `RESOURCE_TYPE_CONTAINER_SERVICE`, used by these very handlers for their not-found envelopes), and the modelled `OperationType` enum declares a value for each of the six calls. An image-scoped call has the same shape as `CreateBucketAccessKey`, which logs its entry against the named `Bucket` because the enum has no member for an access key.

## Evidence

The crate's stated invariant — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:36-37:
```
//! Region), the append-only **operation log** every mutating
//! call writes, and the service's static **Region catalog** — the Regions and
```
and again, as a handler-level rule — src/ops/disable_add_on.rs:11-12:
```
//! and the change is logged in the account's operation log like every other mutating
//! call.
```

The six handlers that break it. `CreateContainerService` inserts the record and answers, with no log write — src/ops/create_container_service.rs:348-353:
```
            let view = service.view(endpoint.as_deref());
            store.insert_container_service(service);
            Ok(CreateContainerServiceOutput {
                container_service: Some(view),
            })
        })
```
`DeleteContainerService` — src/ops/delete_container_service.rs:172-178:
```
            store
                .remove_container_service(&region, &input.service_name)
                .ok_or_else(|| not_found(&input.service_name))?;
            // The record is gone as this call returns, so the modelled result
            // carries nothing to report: `DeleteContainerServiceResult` declares
            // no member at all, which is what this operation's 204 answers.
            Ok(DeleteContainerServiceOutput {})
```
`RegisterContainerImage` — src/ops/register_container_image.rs:213-216:
```
            let registered = service.register_image(&input.label, &input.digest, now);
            Ok(RegisterContainerImageOutput {
                container_image: Some(registered.view(&input.service_name)),
            })
```
(`grep -n 'start_operation\|record_untyped_operation' src/ops/create_container_service.rs src/ops/update_container_service.rs src/ops/create_container_service_deployment.rs src/ops/delete_container_service.rs src/ops/register_container_image.rs src/ops/delete_container_image.rs` → no matches; the same grep over the crate lists 70+ other handlers, e.g. src/ops/create_bucket.rs:323, src/ops/update_bucket.rs:361, src/ops/delete_alarm.rs:172, src/ops/tag_resource.rs:253.)

The crate's own statement of the ONLY reason an entry may be skipped — src/ops/create_container_service_registry_login.rs:235-242:
```
        // And nothing else: no entry is appended to the service's operation log.
        // An entry carries a `resourceType` from the modelled `ResourceType`
        // enum and the `resourceName` of the resource it acted on, and a set of
        // log in credentials is neither — it has no name and the enum has no
        // member for it, so an entry for it could only name a resource this call
        // did not touch. The modelled output carries no `operations` member
        // either, which is what sets this apart from `CreateBucketAccessKey`:
        // that call acts on a named `Bucket` and ANSWERS the log entry it wrote.
```
Both halves of that reason ARE satisfied for a container service: smithy_query(query="shape", shape="ResourceType") → values include `"ContainerService"` (the crate's `RESOURCE_TYPE_CONTAINER_SERVICE`, used at src/ops/delete_container_service.rs:192), and smithy_query(query="shape", shape="OperationType") → values include `"CreateContainerService"`, `"UpdateContainerService"`, `"DeleteContainerService"`, `"CreateContainerServiceDeployment"`, `"RegisterContainerImage"`, `"DeleteContainerImage"`.

What the reader promises — src/ops/get_operations.rs:4 ("Reports the whole operation log of the account, a page at a time, oldest") and smithy_query(query="doc", shape="GetOperations"): "Returns information about all operations."

Test coverage: tests/all/operations_log.rs:12 pins "`GetOperations` reports exactly the operation-log [entries] ..." and its `Step` enum (lines 76-107) drives only static-IP, disk, instance, port, domain and delete-instance calls; tests/all/operations_for_resource.rs does the same per resource. The container suites (tests/all/container_service_lifecycle.rs, container_service_update.rs, container_service_deployment.rs, container_service_delete.rs, container_image_registry.rs) make no `GetOperations`/`GetOperationsForResource` call at all, so nothing pins the empty log and no test states a reason for it — unlike the bucket, distribution, load balancer, key pair and GUI-session suites, which all assert their entries.

## Steps to Reproduce

1. Call CreateContainerService with serviceName="svc", power="nano", scale=1 — HTTP 200, a container service is stored.
2. Call GetOperationsForResource with resourceName="svc" — the answer is an EMPTY operations list, because src/ops/create_container_service.rs wrote no entry.
3. Call CreateContainerServiceDeployment, UpdateContainerService, RegisterContainerImage and finally DeleteContainerService for "svc", then call GetOperations — not one of those five calls appears anywhere in the account's log.
4. Repeat the same sequence for a bucket (CreateBucket → UpdateBucket → DeleteBucket) and read GetOperations: every call is reported, each with an `operationType` from the modelled enum.
5. Read src/state.rs:36-37 ("the append-only operation log every mutating call writes") and src/ops/disable_add_on.rs:11-12 ("logged in the account's operation log like every other mutating call") next to the six handlers named above, which write nothing — and src/ops/create_container_service_registry_login.rs:235-242, whose stated reason for skipping the log (no name, no `ResourceType` member) does not hold for a container service.
