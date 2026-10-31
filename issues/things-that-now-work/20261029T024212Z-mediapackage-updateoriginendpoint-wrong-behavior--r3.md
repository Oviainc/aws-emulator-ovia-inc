Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# An update that strips the startover window leaves the endpoint's unfinished harvest jobs harvesting a source the crate's own rule says retains nothing

_Filed: 2026-10-29T02:42:12Z_
_Operation: UpdateOriginEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule about harvest jobs and the startover window: a harvest exports content the endpoint has already published, which the endpoint keeps only while its startover (time-shift) window retains it, so an endpoint with startover playback DISABLED has nothing for a job to harvest. Two of the three code paths that can make that true act on it:

* `CreateHarvestJob` refuses a submission against a stored endpoint whose window is disabled (`startover_required("originEndpointId")`).
* `DeleteOriginEndpoint` — the other way the harvestable source goes away — calls the crate's existing `ComAmazonawsMediapackageMediapackageState::fail_harvests_of_origin_endpoint` helper so every job that has not reached a terminal status is settled into `FAILED` instead of "reporting work in progress ... for a source this service no longer holds".

`UpdateOriginEndpoint` is the third path: it assigns `record.startover_window_seconds = input.startover_window_seconds`, and an update that sends 0 — or simply omits the member, which `apply_origin_endpoint_defaults` fills with the disabled default 0 — leaves the stored endpoint retaining nothing. The handler neither refuses that while unfinished jobs name the endpoint nor calls the `fail_harvests_of_origin_endpoint` helper the crate already has, so those jobs keep reporting `IN_PROGRESS` against an endpoint that, by the crate's own rule, now retains nothing to harvest, and a `CreateHarvestJob` submitted one request later against the very same endpoint is refused. The window in which that is observable is the one the stored due instant leaves open (`HARVEST_SETTLE_AFTER`, 500 ms, and arbitrarily long for a caller that holds the due instant open the way the suite does), which is why this is LOW rather than higher: the job's eventual terminal status is `FAILED` either way. The defect is that the same loss of harvestable content is handled on one write path and silently ignored on the other.

## Evidence

The rule, `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_harvest_job.rs:296-304`:

    // A harvest exports content the endpoint has ALREADY published, which an endpoint
    // keeps only for as long as its startover (time-shift) window retains it: an
    // endpoint with startover playback disabled retains nothing, so there is nothing
    // for the job to harvest and the submission is refused
    if !endpoint.startover_enabled() {
        return Err(crate::state::startover_required("originEndpointId").into());
    }

The sibling write path that DOES act on the loss, `src/ops/delete_origin_endpoint.rs:161-169`:

    // A harvest exports content this endpoint has published, so a job still harvesting
    // it when it is removed can never finish: the removal settles those jobs into their
    // terminal failure status instead of leaving them reporting work in progress against
    // a source this service no longer holds.
    ...
    state.fail_harvests_of_origin_endpoint(&region, &input.id);

and the helper it calls, `src/state.rs:2292`:

    pub fn fail_harvests_of_origin_endpoint(&mut self, region: &str, origin_endpoint_id: &str) {

The path that skips it, `src/ops/update_origin_endpoint.rs:360-365`:

    record.description = input.description.take();
    record.authorization = input.authorization.take();
    record.origination = input.origination.take();
    record.startover_window_seconds = input.startover_window_seconds;
    record.time_delay_seconds = input.time_delay_seconds;
    record.whitelist = input.whitelist.take().unwrap_or_default();

Nothing in the rest of `handle` (lines 189-407) reads the harvest-job store or calls `fail_harvests_of_origin_endpoint`, and nothing refuses the write.

That an update really does disable startover is pinned by `tests/all/harvest_startover.rs:328-342`:

    Step {
        case: "updated to a window of zero seconds",
        startover: Some(0),
        harvestable: false,
    },
    ...
    Step {
        case: "updated with the member left out",
        startover: None,
        harvestable: false,
    },

Test coverage: `tests/all/harvest_startover.rs::the_window_the_stored_endpoint_carries_now_is_what_decides` covers only what a LATER submission is answered; `tests/all/harvest_source_deleted.rs` covers only the delete path's effect on existing jobs. No test exercises an existing job across an update that disables the window.

## Steps to Reproduce

1. CreateChannel with Id="c1".
2. CreateOriginEndpoint with Id="e1", ChannelId="c1", hlsPackage {}, startoverWindowSeconds=86400.
3. CreateHarvestJob with Id="j1", OriginEndpointId="e1" and a valid window/destination; it is accepted and DescribeHarvestJob reports Status=IN_PROGRESS (hold the stored due instant open as tests/all/harvest_jobs.rs::hold_transitions_open does, so the window is deterministic).
4. PUT /origin_endpoints/e1 with body {"hlsPackage": {}} — no startoverWindowSeconds — and observe 200 with startoverWindowSeconds 0, i.e. startover playback disabled.
5. GET /harvest_jobs/j1: the job still reports Status=IN_PROGRESS against an endpoint that now retains nothing, where DELETE /origin_endpoints/e1 would have settled it to FAILED via fail_harvests_of_origin_endpoint.
6. POST /harvest_jobs for a new id against "e1": refused 422 with errors.originEndpointId = ["must have startover enabled"], the same endpoint the in-flight job is still reported as harvesting.
