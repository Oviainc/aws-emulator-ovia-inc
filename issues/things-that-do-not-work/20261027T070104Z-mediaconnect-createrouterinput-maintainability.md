Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# The tier-bounds-the-bitrate rule is implemented twice in state.rs, once for router inputs and once for router outputs

_Filed: 2026-10-27T07:01:03Z_
_Operation: CreateRouterInput | Class: maintainability_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`router_input_bitrate_rejection` and `router_output_bitrate_rejection` are the same computation written out twice: resolve the tier's ceiling from a five-entry Mbps ladder, refuse an unknown tier, refuse a non-positive bitrate, refuse a bitrate past the ceiling. The two ceiling catalogs (`router_input_tier_bitrate_ceiling`, `router_output_tier_bitrate_ceiling`) differ only in the enum prefix of their keys (`INPUT_*` vs `OUTPUT_*`) and carry the identical 20/50/100/250/500 Mbps ladder; the two rejection functions differ only in the noun inside the unknown-tier sentence. Four handlers reach them (CreateRouterInput, UpdateRouterInput, CreateRouterOutput, UpdateRouterOutput), so the two copies can drift — a bound or message fixed on one side silently leaves the other behind. Folding them onto one helper parameterised by the tier prefix and the resource noun keeps every response byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:3470-3489:
```
pub fn router_input_bitrate_rejection(tier: &str, maximum_bitrate: i64) -> Option<String> {
    let Some(ceiling) = router_input_tier_bitrate_ceiling(tier) else {
        return Some(format!(
            "Tier {tier:?} is not a router input tier this service offers."
        ));
    };
    if maximum_bitrate <= 0 {
        return Some(format!(
            "MaximumBitrate is a number of bits per second and must be positive; this \
             request asks for {maximum_bitrate}."
        ));
    }
    if maximum_bitrate > ceiling {
        return Some(format!(
            "MaximumBitrate {maximum_bitrate} bps exceeds the {ceiling} bps the {tier} tier \
             allows."
        ));
    }
    None
}
```
Copy 2 — src/state.rs:4035-4054, the same four branches and the same two message strings:
```
pub fn router_output_bitrate_rejection(tier: &str, maximum_bitrate: i64) -> Option<String> {
    let Some(ceiling) = router_output_tier_bitrate_ceiling(tier) else {
        return Some(format!(
            "Tier {tier:?} is not a router output tier this service offers."
        ));
    };
    if maximum_bitrate <= 0 { ... }
    if maximum_bitrate > ceiling { ... }
    None
}
```
The two catalogs behind them carry the same ladder — src/state.rs:3446-3456 (`"INPUT_20" => 20, "INPUT_50" => 50, "INPUT_100" => 100, "INPUT_250" => 250, "INPUT_500" => 500`) and src/state.rs:4012-4022 (`"OUTPUT_20" => 20, ... "OUTPUT_500" => 500`).

Callers: src/ops/create_router_input.rs:215, src/ops/update_router_input.rs:334, src/ops/create_router_output.rs:209, src/ops/update_router_output.rs:339.

Existing tests that pin both copies (responses must stay unchanged): tests/all/router_outputs.rs:958 `the_tier_bounds_the_maximum_bitrate`, tests/all/update_router_output.rs:496 `the_tier_bounds_the_maximum_bitrate_an_update_may_set` (asserts against `mc::router_output_tier_bitrate_ceiling`), tests/all/update_router_input.rs:586 `the_tier_bounds_the_maximum_bitrate_an_update_may_set` (asserts against `mc::router_input_tier_bitrate_ceiling`), tests/all/create_router_input.rs:202 `the_routing_scope_and_tier_are_stored_and_a_name_is_not_unique`.

## Steps to Reproduce

1. Read services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:3446-3489 and 4012-4054 side by side: the ceiling ladder and the three rejection branches, including both message strings, are duplicated.
2. Change one branch (for example the non-positive-bitrate sentence) in `router_input_bitrate_rejection` and observe that nothing makes the router-output copy follow it; the two caller pairs then refuse the same input with different wording.
3. Confirm the behaviour that must not change by running the four tests named in the evidence.
