Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# RegisterContainerImage refuses every request, so no container image can ever be registered to a container service

_Filed: 2026-10-24T09:56:37Z_
_Operation: RegisterContainerImage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves the container service and then unconditionally returns Lightsail's `DoesNotExist` refusal for the digest — there is no code path that can store an image. The modelled output member `RegisterContainerImageResult$containerImage` is therefore unreachable, `GetContainerImages` can only ever report an empty list, and nothing can create the `:service.label.version` image names that `DeleteContainerImage` and a container-service deployment reference.

Expected (model + service semantics): `RegisterContainerImage(serviceName, label, digest)` returns 200 with a `containerImage` carrying the service-assigned `image` name (`:<service>.<label>.<version>`), the `digest` the request carried and a `createdAt`; a subsequent `GetContainerImages` for that service lists it, and `DeleteContainerImage` can remove it.

Actual: every well-formed request against an existing service answers HTTP 400 / `NotFoundException` with `code: DoesNotExist` and the message `Image with digest "..." not found.`, and the service's registry stays empty forever. The stated reason is that this emulator serves no container-image push data plane, so no digest can genuinely be present — but the consequence is that an entire modelled capability (the private image registry: register → list → reference in a deployment → delete) cannot be exercised at all, and a client following the documented workflow (`CreateContainerServiceRegistryLogin` → push → `RegisterContainerImage`) gets a not-found for an image it just pushed.

## Evidence

Model: `smithy_query(query="operation", operation="RegisterContainerImage")` — output `{"containerImage": "ContainerImage"}`, i.e. a successful registration is the documented outcome; `GetContainerImages` reports "the container images that are registered to your Amazon Lightsail container service".

Handler: `src/ops/register_container_image.rs` lines 179-201 — after `store.container_service(...)` resolves, the closure's only remaining statement is `Err(digest_does_not_exist(&input.digest))`; there is no insert into `StoredContainerService::images`, which is initialised to `Vec::new()` by `create_container_service.rs` (line ~330) and is never pushed to anywhere in the crate.

State: `src/state.rs` line ~6254 — "`RegisterContainerImage` refuses every digest (see that handler)", confirming the registry is unfillable by design.

Test coverage: `tests/all/container_image_registry.rs` lines 377-415 (`registering_a_digest_nothing_pushed_is_refused`) asserts the current always-refuse behaviour, so the capability gap is locked in rather than flagged.

## Steps to Reproduce

1. Boot the emulator and point a Lightsail client at it (region us-east-1).
2. CreateContainerService(serviceName='svc', power='nano', scale=1) and wait for state READY.
3. RegisterContainerImage(serviceName='svc', label='web', digest='sha256:ab...') with any digest value.
4. Observe HTTP 400 NotFoundException / code=DoesNotExist, message 'Image with digest "..." not found.', instead of 200 with a containerImage.
5. GetContainerImages(serviceName='svc') — the list is empty; no sequence of calls against this emulator can ever make it non-empty.
