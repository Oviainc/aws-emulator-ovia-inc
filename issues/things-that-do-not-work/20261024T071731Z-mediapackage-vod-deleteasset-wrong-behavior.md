Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Bodyless 204 delete responses omit the content-length: 0 framing header AWS states

_Filed: 2026-10-24T07:17:31Z_
_Operation: DeleteAsset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the three MediaPackage VOD deletes answer with no body (`DeleteAsset`, `DeletePackagingConfiguration`, `DeletePackagingGroup` all serve HTTP 204 in this crate). A response with no body still has a length and AWS states it: these answers carry `content-length: 0`.

Actual: this crate's `src/service.rs` is the unedited scaffold — it implements only the five default seams (`can_handle`, `override_operation`, `override_refusal`, `override_routing_error`, `override_framework_error`) and does NOT override the protocol, so the restJson1 serializer's output is served as-is. That serializer leaves the framing header off an empty body, so the emulator's 204 answers carry no `content-length` at all and leave the length to the transport to frame.

The sibling MediaPackage (live) crate had exactly this divergence filed and fixed: `services/service-com-amazonaws-mediapackage-mediapackage/src/service.rs` documents it ("A response with no body still has a LENGTH, and AWS states it: these answers carry `content-length: 0`. The protocol's serializer leaves the header off…") and corrects it with `fn frame_empty_body` wired through an `override_protocol`/`ServerProtocol` impl (issue `20261023T044637Z-mediapackage-wire-divergences-...`, cause: "204 tag responses omit Content-Length"). MediaPackageV2 (`src/service.rs` inserts `content-length: 0`) and several other restJson1 crates carry the same correction. MediaPackage VOD has the same empty-body answers and none of the correction.

Consequence: a client, proxy or SDK transport that keys on a framing header being present on a bodyless response reads the emulator's reply differently from the service's. (Over plain HTTP/1.1 the vendored writer strips `content-length` from a 204, so the difference is observable on the in-process/SDK-connector paths — which is the same scope in which the sibling fix is asserted.) The same correction will be needed for `TagResource`/`UntagResource` (modeled 204) once those stubs are implemented.

## Evidence

Spec/recording: `DeleteAsset`, `DeletePackagingConfiguration` and `DeletePackagingGroup` have empty output shapes, and this crate already reconciles their success status to the recorded `204 No Content` (`src/ops/delete_asset.rs` line ~148, `src/ops/delete_packaging_configuration.rs` line ~149, `src/ops/delete_packaging_group.rs` line ~173). The `content-length: 0` requirement for an AWS bodyless answer is the recorded behavior documented and fixed in the sibling crate: `services/service-com-amazonaws-mediapackage-mediapackage/src/service.rs` lines ~82-106 (module docs) and ~358-413 (`frame_empty_body` applied in `serialize_output`/`serialize_error`), pinned by `services/service-com-amazonaws-mediapackage-mediapackage/tests/all/empty_response_framing.rs` (`assert_empty_and_framed` asserts an empty body plus `content-length: 0`).

Handler/service: `services/service-com-amazonaws-mediapackagevod-mediapackagevod/src/service.rs` is the generated scaffold with every seam returning `None` and no `ServerProtocol` impl, so nothing frames the empty body.

Tests: the VOD crate's delete tests assert only the status (`tests/all/packaging_group_deletion.rs` line 36 and `tests/all/packaging_configuration_deletion.rs` line 42: `const DELETE_SUCCESS: u16 = 204;`); a grep for `content-length` across `tests/all` of this crate returns nothing, so the framing is unasserted.

## Steps to Reproduce

1. Boot the emulator (or drive it in-process through the router, as this crate's tests do) and create a packaging group `g1` plus an asset `a1` in it.
2. Issue `DELETE /assets/a1` and inspect the response headers: observe HTTP 204 with an empty body and NO `content-length` header.
3. Do the same for `DELETE /packaging_configurations/{id}` and `DELETE /packaging_groups/{id}`: same missing framing header.
4. Compare with the MediaPackage (live) emulator's 204 tagging answers, which state `content-length: 0` via `frame_empty_body` in its `src/service.rs`.
