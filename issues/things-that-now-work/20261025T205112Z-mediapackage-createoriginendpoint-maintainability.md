Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 558d123cf10a98531c236bb7b81525d860a40117

# The duration-floor rule is implemented twice in bound_origin_endpoint_durations (local `judge` plus an inline copy for CMAF child manifests)

_Filed: 2026-10-25T20:51:12Z_
_Operation: CreateOriginEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `558d123cf10a98531c236bb7b81525d860a40117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::bound_origin_endpoint_durations` — the single function both `CreateOriginEndpoint` (services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_origin_endpoint.rs:232) and `UpdateOriginEndpoint` (src/ops/update_origin_endpoint.rs:259) call to enforce the non-negative floor on every seconds-valued member — contains TWO independent implementations of that one rule.

The first is the nested helper `judge`, used for the fixed-path members of `hlsPackage`, `dashPackage`, `cmafPackage` and `mssPackage`. The second is an inline re-write of exactly the same predicate and the same refusal constructor, 80 lines below, for the CMAF child HLS manifests, whose only difference is that the member path carries a list index. The two copies can drift: a change to the comparison (e.g. the floor's strictness) or to the refusal built from it has to be made twice, and nothing in the crate ties them together. The second copy can call the first (`judge(&[(&path, seconds)])`, or `judge` widened to take an owned path), which leaves every response byte-identical because both copies already produce the same key and the same sentence from `duration_refusal`/`MIN_DURATION_SECONDS`.

This is purely a duplication of logic — no behavioural change is requested, and both copies are already pinned by the existing suite.

## Evidence

Copy 1 — the helper, services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:1201-1209:
```
    /// One group of members whose paths are fixed: refuse the first negative one.
    fn judge(members: &[(&str, Option<i32>)]) -> Result<(), ServiceError> {
        for (member, seconds) in members {
            if seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS) {
                return Err(duration_refusal(member));
            }
        }
        Ok(())
    }
```

Copy 2 — the same rule re-written inline in the SAME function, src/state.rs:1284-1298:
```
        for (index, manifest) in package.hls_manifests.iter().flatten().enumerate() {
            for (member, seconds) in [
                ("playlistWindowSeconds", manifest.playlist_window_seconds),
                (
                    "programDateTimeIntervalSeconds",
                    manifest.program_date_time_interval_seconds,
                ),
            ] {
                if seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS) {
                    return Err(duration_refusal(&format!(
                        "cmafPackage.hlsManifests[{index}].{member}"
                    )));
                }
            }
        }
```
The predicate (`seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS)`) and the refusal (`duration_refusal(..)`) are identical; only the construction of the member path differs.

Both callers go through this one function, which is itself documented as the single home of the rule (src/state.rs:1185-1189): "Both handlers call this, so the floor one enforces cannot drift from the other's: an update must not be able to store a window a create of it would have refused."

Test coverage that pins both copies (so a consolidation is verifiable without response changes) — services/service-com-amazonaws-mediapackage-mediapackage/tests/all/origin_endpoint_durations.rs:
* line 97: `path: "hlsPackage.playlistWindowSeconds"` (exercises copy 1)
* line 180: `path: "cmafPackage.hlsManifests[0].playlistWindowSeconds"` and line 190: `path: "cmafPackage.hlsManifests[0].programDateTimeIntervalSeconds"` (exercise copy 2)

## Steps to Reproduce

1. Open services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs and read `bound_origin_endpoint_durations` (lines 1193-1313).
2. Note the nested `judge` helper at lines 1202-1209: it compares each member against `MIN_DURATION_SECONDS` and returns `duration_refusal(member)`.
3. Note lines 1284-1298 inside the same function: the identical comparison and `duration_refusal(..)` call are written a second time for the CMAF child manifests, with only the member path differing.
4. Confirm both paths are live from the handlers: src/ops/create_origin_endpoint.rs:232 and src/ops/update_origin_endpoint.rs:259 both call this one function.
5. Confirm both copies are already covered on the wire by tests/all/origin_endpoint_durations.rs rows at lines 97 and 180/190, so routing the second copy through `judge` must leave every status, key and message unchanged.
