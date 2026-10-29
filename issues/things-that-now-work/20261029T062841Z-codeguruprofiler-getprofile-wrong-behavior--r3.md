Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# GetProfile serves a single stored profile for the requested range, dropping every other profile the same aggregated window holds

_Filed: 2026-10-29T06:28:41Z_
_Operation: GetProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetProfile` resolves the requested time range to exactly ONE stored agent profile — the most recently received one — and hands its bytes back. Every other profile the group holds inside the requested range (including other profiles collected into the SAME aggregation window) is silently dropped from the answer.

The model documents the opposite: an aggregated profile CONTAINS the profiles posted during its aggregation period, and a request whose range covers several aggregated profiles is answered with them combined into one aggregated profile. This crate's own state model agrees with the model — `aggregated_profile_starts` collapses every posted profile of one aligned window into ONE aggregated profile (so `ListProfileTimes` reports one profile time for a window holding three posted profiles), and the sibling reader of the very same data, `BatchGetFrameMetricData`, tallies EVERY profile of a step into that step's value. Only `GetProfile` picks one profile and discards the rest, so the aggregated profile it serves for a window is a strict subset of the data the service stores for that window and reports elsewhere.

Consequence: the documented use case 1 ("use ListProfileTimes to view the time ranges of existing aggregated profiles. Use them in a GetProfile request to return a specific, existing aggregated profile") returns only part of the aggregated profile that `ListProfileTimes` just advertised, and frames/samples that `BatchGetFrameMetricData` counts for the same range are absent from the profile document a caller reads.

## Evidence

The model states the behaviour (smithy_query(query="doc", shape="GetProfile")):
> "If you want to return an aggregated profile for a time range that doesn't align with an existing aggregated profile, then CodeGuru Profiler makes a best effort to combine existing aggregated profiles from the requested time range and return them as one aggregated profile."
> "If aggregated profiles do not exist for the full time range requested, then aggregated profiles for a smaller time range are returned. For example, if the requested time range is from 00:00 to 00:20, and the existing aggregated profiles are from 00:15 and 00:25, then the aggregated profiles from 00:15 to 00:20 are returned."
and smithy_query(query="shape", shape="AggregatedProfileTime"), member `start`:
> "The aggregation profile contains profiles posted by the agent starting at this time for an aggregation period specified by the period property of the AggregatedProfileTime object."

What the handler does — src/ops/get_profile.rs:854-855:
```
            group
                .latest_profile_in(start, end, crate::state::now())
```
and the store reader it calls — src/state.rs:1421-1433:
```
    pub fn latest_profile_in(
        ...
        self.profiles.iter().rfind(|profile| {
```
ONE profile is selected; `served_profile` then re-encodes/bounds that single document (src/ops/get_profile.rs:877-887).

The same crate treats a window's profiles as one aggregate everywhere else:
* src/state.rs:1266-1269 — "every window holding at least one AGGREGATED posted profile is exactly one available aggregated profile, reported by its window start" (`aggregated_profile_starts` dedups the window starts of several profiles).
* src/ops/batch_get_frame_metric_data.rs:459-463 — every profile of a step contributes:
```
        for (step, document) in &documents {
            let (frame, root) = tally(document, &metric.frame_name, &thread_states, &mut budget)?;
            named[*step] += frame;
            whole[*step] += root;
        }
```
* tests/all/stored_profile_representation.rs:442-451 pins that rule for the metric path — "Every profile of a step counts towards its frame metric ... the value is the named frame's counts over the counts of ALL the profiles of that step ... where reading only one of the two profiles would report 1 or 0 instead" — i.e. the crate already has a test whose group holds TWO profiles in ONE hourly window.

Test coverage of the current GetProfile behaviour: the single-profile paths (tests/all/instant_comparison_resolution.rs:333-338, tests/all/profile_representations.rs:453, tests/all/served_media_type.rs:185) each store ONE profile and are unaffected. The one assertion that pins "a single profile is returned" out of several is tests/all/profiling_group_lifecycle.rs:1258-1262 — "GetProfile must return the profile posted last" — whose stated reason at :1254 is retention ("the newest profile must still be readable"), not the combining rule the model states.

## Steps to Reproduce

1. CreateProfilingGroup `combine-demo`.
2. PostAgentProfile twice with two DISTINCT readable JSON profile documents (each carrying the `start` member PostAgentProfile requires), with distinct `profileToken`s, so the group retains two profiles whose receipt times fall in the same aligned 5-minute window.
3. Let the aggregation of that window become due (or move the receipts into a past window, as tests/all/profile_times.rs::place_receipts does).
4. GET /profilingGroups/combine-demo/profile?startTime=<window start>&endTime=<window end>: the body is byte-identical to exactly ONE of the two posted payloads; the frames and samples of the other are absent from the served aggregated profile.
5. POST /profilingGroups/combine-demo/frames/-/metrics over the same range with a frameMetric naming a frame that appears only in the DROPPED profile: the metric reports a non-zero value, so the service does hold and report that profile's data for this window — only GetProfile omits it.
6. Compare with GET /profilingGroups/combine-demo/profileTimes?...&period=PT5M, which reports ONE profile time for that window, i.e. one aggregated profile that is documented to contain both posted profiles.
