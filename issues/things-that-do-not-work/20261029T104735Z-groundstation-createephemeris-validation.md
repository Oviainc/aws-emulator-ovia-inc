Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6270af2faafa317884be9e4627608e6b01112bc2

# An azEl ephemeris whose pointing points are out of `dt` order, whose segments overlap, or whose angles are out of range is accepted and reported ENABLED

_Filed: 2026-10-29T10:47:35Z_
_Operation: CreateEphemeris | Class: validation_

> _Note: this issue was identified against the code base at commit `6270af2faafa317884be9e4627608e6b01112bc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEphemeris` takes its validation verdict from `crate::state::ephemeris_invalid_reason`, which applies exactly ONE rule: every validity range must run forwards. Every OTHER rule the model states in prose for azimuth/elevation data is unchecked, so data the model declares unusable is stored, settles to `ENABLED`, becomes the satellite's `currentEphemeris`, and is then read as "the pointing data in force" by the availability search.

Three symptoms of the same missing gate (one class, one operation, consolidated here):

1. `dt` values not in ascending order (or duplicated). `crate::state::az_el_passes` walks `segment.az_el_list` in the SUPPLIED order and builds a run whose `start` is the first usable point's instant and whose `end` is the LATEST one it walked to, then drops any run with `end <= start` (`passes.retain(|pass| pass.end > pass.start)`). So a segment whose five points carry descending `dt` passes validation, reports `status: ENABLED` and `invalidReason` absent, and then silently yields NO pass at all — `ListContacts` with `statusList: ["AVAILABLE"]` answers an empty page for an ephemeris the same service reports as in force, and a caller has nothing in any response telling it why.

2. Segments that overlap or are out of chronological order. `az_el_passes` emits each segment's runs independently; `pointing_passes` then DISCARDS a candidate that overlaps a pass already kept (state.rs:5496-5500). So overlapping segments are accepted at create and their pointing data is silently thrown away on read, instead of the create refusing the data the model says must not overlap.

3. `az`/`el` outside the documented per-unit ranges. `el` is only ever compared against `HORIZON_ELEVATION` (0.0) and `az` is never read at all, so `el: 500` with `angleUnit: DEGREE_ANGLE` is stored, enabled, and surfaces verbatim as an offered window's `maximumElevation` (`Elevation { value: pass.peak, unit: segments.angle_unit.clone() }`, state.rs:5507-5510).

This is not a feature the crate never attempts: the verdict helper, the `EphemerisInvalidReason` constant, the `VALIDATING -> INVALID` settle and a test row for the one rule that IS applied all already exist. The gate is simply one rule wide where the model states several.

## Evidence

Model states the rules (each `must`, verbatim):
* `smithy_query(query="shape", shape="TimeAzEl")`, member `dt`: "Time offset in atomic seconds from the segment's reference epoch." / "All `dt` values within a segment **must be in ascending order with no duplicates**."
* `smithy_query(query="shape", shape="AzElSegments")`, member `azElSegmentList`: "List of azimuth elevation segments." / "Must contain between 1 and 100 segments. **Segments must be in chronological order with no overlaps**."
* `smithy_query(query="shape", shape="TimeAzEl")`, member `el`: "Valid ranges by unit: `DEGREE_ANGLE`: -90 to 90 degrees, where 0 is the horizon, 90 is zenith, and negative values are below the horizon; `RADIAN`: -π/2 to π/2 radians…"; member `az`: "`DEGREE_ANGLE`: -180 to 360 degrees, measured clockwise from true north; `RADIAN`: -π to 2π radians…"
* The model even names the failure codes for exactly these conditions — `smithy_query(query="shape", shape="EphemerisErrorCode")` values include `TIME_AZ_EL_ITEMS_OUT_OF_ORDER`, `AZ_EL_SEGMENTS_OUT_OF_ORDER`, `AZ_EL_SEGMENT_TIMES_OVERLAP`, `TIME_AZ_EL_EL_DEGREE_RANGE_INVALID`, `TIME_AZ_EL_AZ_DEGREE_RANGE_INVALID`, `TIME_AZ_EL_ANGLE_UNITS_INVALID` — beside the one the crate does implement, `AZ_EL_SEGMENT_END_TIME_BEFORE_START_TIME`. None of these constraints is a modelled trait (`TimeAzElList` is only `@length(min: 5)`, `AzElSegmentList` only `@length(min: 1, max: 100)`), so the boundary validator does not apply them and the handler is the only gate.

Handler side — the gate is one rule wide:
* `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:3560-3572`: "/// Why the data `data` supplies cannot be used, as an `EphemerisInvalidReason`, or `None` /// when nothing in it is wrong. /// /// A validity range must run forwards: one whose end does not follow its start describes /// no interval at all, so the data is refused as [`EPHEMERIS_TIME_RANGE_INVALID`] rather /// than enabled." and the body: `ephemeris_validity_ranges(data).iter().any(|(start, end)| end <= start).then_some(EPHEMERIS_TIME_RANGE_INVALID)` — nothing else is examined.
* `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_ephemeris.rs:259-262`: "// The validation verdict is taken here, from the data as supplied, and // applied when the validation falls due — nothing about the data changes in // between." / `invalid_reason: crate::state::ephemeris_invalid_reason(&data),`

Consumer side — the unvalidated data is then used as pointing data:
* `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:3651-3693` `fn az_el_passes(...)`: `for point in &segment.az_el_list { … let at = epoch + point.dt; … Some(open) => AzElPass { start: open.start, end: at, peak: open.peak.max(point.el) } … }` followed by `passes.retain(|pass| pass.end > pass.start);` — the run's `end` is whatever point came LAST in the supplied order, and `point.el` is only compared with `HORIZON_ELEVATION`.
* `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:5459` `pointing_passes` admits it: `.filter(|ephemeris| ephemeris.status == EPHEMERIS_ENABLED)`; `state.rs:5496-5500` silently drops an overlapping candidate: `if after > 0 && passes[after - 1].end_time.as_secs_f64() >= pass.start { continue; }`; `state.rs:5507-5510` reports the unbounded peak as `maximum_elevation`.

Test coverage proves the mechanism exists and that only the one rule is pinned: `tests/all/ephemeris_catalog.rs:712-731` is the whole status table for invalid data — rows "a tle validity range that does not run forwards" and "an azEl segment that does not run forwards", both `settled_status: EPHEMERIS_INVALID`. No row covers `dt` order, segment overlap or angle range. The test helper itself reads these same model sentences (`tests/all/ephemeris_catalog.rs:247-249`: "Each segment carries the five time-tagged points `AzElSegment$azElList` requires (\"must contain at least five points to support 4th order Lagrange interpolation\" — `smithy_query(query="shape", shape="AzElSegment")`)").

## Steps to Reproduce

1. Start the emulator with a satellite on-boarded to a ground station, e.g. GROUNDSTATION_SATELLITES="25994=Ohio 1" (as tests/all/ephemeris_catalog.rs does).
2. POST /ephemeris with name="descending", enabled=true, satelliteId=<that satellite>, and ephemeris={"azEl":{"groundStation":"Ohio 1","data":{"azElData":{"angleUnit":"DEGREE_ANGLE","azElSegmentList":[{"referenceEpoch":1800000000,"validTimeRange":{"startTime":1800000000,"endTime":1800086400},"azElList":[{"dt":40,"az":10,"el":20},{"dt":30,"az":11,"el":21},{"dt":20,"az":12,"el":22},{"dt":10,"az":13,"el":23},{"dt":0,"az":14,"el":24}]}]}}} — the `dt` values descend, which the model says must ascend. The create answers 200.
3. Wait out EPHEMERIS_VALIDATION_SETTLES_AFTER and GET /ephemeris/{ephemerisId}: status is ENABLED and invalidReason is absent, i.e. the service declares the trajectory usable.
4. GET /satellite/{satelliteId}: currentEphemeris names this ephemeris, so the satellite's trajectory is reported as coming from it.
5. POST /contacts with statusList ["AVAILABLE"], groundStation "Ohio 1", the satelliteArn and a missionProfileArn, over a window covering 1800000000..1800086400: the page is empty — az_el_passes built a run with end < start and retain() dropped it. Nothing in any response says the pointing data was unusable.
6. Repeat with two azElSegmentList entries whose validTimeRanges overlap (e.g. 1800000000..1800086400 and 1800040000..1800120000): the create is still accepted and ENABLED, and the availability search silently reports only the first segment's pass (pointing_passes drops the overlapping candidate).
7. Repeat with a single ascending segment but "el": 500 on every point and angleUnit DEGREE_ANGLE: the create is accepted, and the AVAILABLE window returned by POST /contacts carries maximumElevation {"value": 500.0, "unit": "DEGREE_ANGLE"}, outside the -90..90 the model documents for that unit.
