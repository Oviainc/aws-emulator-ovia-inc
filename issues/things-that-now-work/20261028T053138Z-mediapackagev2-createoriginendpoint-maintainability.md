Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# The per-family manifest-count array is built twice, once in each origin-endpoint writer

_Filed: 2026-10-28T05:31:38Z_
_Operation: CreateOriginEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::manifest_set_violation` is the one place the manifest-set rules live, and it takes the counts as `[usize; ManifestFamily::ALL.len()]` — an array whose POSITIONS carry meaning: the helper zips it against `ManifestFamily::ALL` to pick the container/family conflict discriminator. Building that array is a computation implemented twice, once per writer, with no shared definition: `create_origin_endpoint.rs::manifest_counts` and `update_origin_endpoint.rs::manifest_counts` are element-for-element identical.

Because the ORDER is the contract, the two copies can drift silently: reordering `ManifestFamily::ALL`, or adding a family, requires editing both, and a copy left in the old order would hand `manifest_set_violation` a DASH count where it reads an MSS count, so the wrong `ValidationExceptionType` is reported (or a legal set refused) without any compile error. One shared builder — taking the four optional lists (or their lengths) and emitting the array beside `ManifestFamily::ALL`, where the order is declared — removes the second copy. Every response and every existing test stays byte-identical: the arrays produced are the same values the two copies produce today.

## Evidence

Copy 1 — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/create_origin_endpoint.rs:224-233:
  "/// How many manifests the request carries per family, in
   /// [`crate::state::ManifestFamily::ALL`] order.
   fn manifest_counts(input: &CreateOriginEndpointInput) -> [usize; 4] {
       [
           input.hls_manifests.as_ref().map_or(0, Vec::len),
           input.low_latency_hls_manifests.as_ref().map_or(0, Vec::len),
           input.dash_manifests.as_ref().map_or(0, Vec::len),
           input.mss_manifests.as_ref().map_or(0, Vec::len),
       ]
   }"

Copy 2 — src/ops/update_origin_endpoint.rs:219-230:
  "/// How many manifests the request carries per family, in
   /// [`crate::state::ManifestFamily::ALL`] order. An update carries the WHOLE manifest set …
   fn manifest_counts(input: &UpdateOriginEndpointInput) -> [usize; 4] {
       [
           input.hls_manifests.as_ref().map_or(0, Vec::len),
           input.low_latency_hls_manifests.as_ref().map_or(0, Vec::len),
           input.dash_manifests.as_ref().map_or(0, Vec::len),
           input.mss_manifests.as_ref().map_or(0, Vec::len),
       ]
   }"

Why the order is load-bearing — src/state.rs:3044-3099:
  "pub fn manifest_set_violation(
       container: &str,
       counts: [usize; ManifestFamily::ALL.len()],
       on: ManifestSetOn,
   ) -> Option<ManifestSetViolation> {"
  … "for (family, count) in ManifestFamily::ALL.into_iter().zip(counts) {"
and the shared-rule intent the helper already states (src/state.rs:3035-3038):
  "This is the same rule for `CreateOriginEndpoint` and `UpdateOriginEndpoint`: both carry the whole manifest set and the same required `ContainerType` …"

Tests that cover both callers and must stay unchanged: tests/all/manifest_count_range.rs (NUM_MANIFESTS_LOW / NUM_MANIFESTS_HIGH on create and update) and tests/all/control_plane_validation.rs (the container-vs-family discriminator rows, e.g. the `TS`/`CMAF`/`ISM` cases around control_plane_validation.rs:893-911).

## Steps to Reproduce

1. Open src/ops/create_origin_endpoint.rs:224-233 and src/ops/update_origin_endpoint.rs:219-230 and compare the two `manifest_counts` bodies — they are identical, and both encode the `ManifestFamily::ALL` ordering independently.
2. Note that src/state.rs:3081 (`for (family, count) in ManifestFamily::ALL.into_iter().zip(counts)`) is the only consumer, so a reordering of `ManifestFamily::ALL` changes the meaning of both arrays while neither copy fails to compile.
3. Replace both with one shared builder declared beside `ManifestFamily::ALL` in src/state.rs and re-run `cargo test -p service-com-amazonaws-mediapackagev2-mediapackagev2` — manifest_count_range.rs and control_plane_validation.rs must still pass unchanged.
