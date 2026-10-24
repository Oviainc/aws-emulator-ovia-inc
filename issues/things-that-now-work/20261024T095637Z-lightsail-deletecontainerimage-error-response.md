Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# DeleteContainerImage answers 204 success for an image the service's registry does not hold, and never returns the modelled NotFoundException

_Filed: 2026-10-24T09:56:37Z_
_Operation: DeleteContainerImage | Class: error-response_

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler treats the delete as unconditionally idempotent: it resolves the container service, then calls `service.images.retain(|image| image.image != input.image)` and answers 204 whether or not the registry held that image. The modelled `NotFoundException` is unreachable for this operation, because the only other refusal the handler can produce — a service name that names nothing in the region — is answered with `InvalidInputException` instead.

Expected: deleting an image name the service's registry does not hold is refused with the modelled `NotFoundException` (the image was not found), the same way the sibling `RegisterContainerImage` handler refuses an unpushed digest with a not-found (its own comment: "AWS answers an unpushed digest with `NotFoundException`/`DoesNotExist`"). The two cannot both be true: if the missing-SERVICE case is `InvalidInputException` ("Service \"x\" does not exist."), then the `NotFoundException` the model declares for this operation is the missing-IMAGE case, and the handler never produces it.

Actual: `DeleteContainerImage(serviceName=<existing>, image=<anything>)` answers HTTP 204 with an empty body for every image name — including names that were never registered and names that are not even in the registered (`:service.label.version`) form — so a caller cannot tell a real deletion from a no-op and the documented failure mode is unobservable.

## Evidence

Model: `smithy_query(query="operation", operation="DeleteContainerImage")` — errors include `{"shape": "NotFoundException", "httpError": 404}`; the operation's only input members are `serviceName` and `image`, so the modelled not-found covers one of the two, and the handler answers the service case with `InvalidInputException`.

Handler: `src/ops/delete_container_image.rs` lines 164-181 — `service.images.retain(...)` then `Ok(DeleteContainerImageOutput {})` with `SUCCESS_STATUS = 204`; lines 184-196 show the missing-service refusal is `InvalidInputException`, leaving `DeleteContainerImageErrorModeled::NotFoundException` constructed nowhere in the crate.

Sibling claim about the service's behaviour: `src/ops/register_container_image.rs` lines 196-199 — "AWS answers an unpushed digest with `NotFoundException`/`DoesNotExist`".

Test coverage: `tests/all/container_image_registry.rs` lines 417-452 asserts the current unconditional-idempotence behaviour ("a name the registry does not hold leaves it as it was"), i.e. the divergence is asserted rather than caught.

## Steps to Reproduce

1. Boot the emulator and point a Lightsail client at it (region us-east-1).
2. CreateContainerService(serviceName='svc', power='nano', scale=1).
3. DeleteContainerImage(serviceName='svc', image=':svc.web.1') — an image nothing registered.
4. Observe HTTP 204 with an empty body, instead of the modelled NotFoundException for an image the registry does not hold.
5. Repeat with image='nginx:latest' (a public-registry form the service cannot hold at all) and observe the same 204.
