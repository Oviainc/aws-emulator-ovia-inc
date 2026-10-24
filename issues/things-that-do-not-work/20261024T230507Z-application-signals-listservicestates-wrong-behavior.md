Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# ListServiceStates reports the change event's Region as the READER's region, contradicting the rule ListEntityEvents applies to the same record

_Filed: 2026-10-24T23:05:07Z_
_Operation: ListServiceStates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both `ListEntityEvents` and `ListServiceStates` report a `ChangeEvent` for the same recorded changes (an SLO's `CreatedTime`/`LastUpdatedTime`), and both build it through the single `change_event` helper, whose doc claims the two reads "cannot differ on what an event of this emulator carries". They do differ on `Region`:

- `ListEntityEvents` passes `record_region(arn)` — the Region of the ARN of the record the change was read off — and the helper's doc argues explicitly that this is the right value, "the record's own rather than the reader's".
- `ListServiceStates` passes the Region read off the CALLING request's SigV4 credential scope (`region_of(ctx.parts())`), even though the instant it reports came from the same stored SLO records, whose ARNs carry the writer's Region.

So an SLO created by a caller signing in `us-west-2` is reported by `ListEntityEvents` as a change in `us-west-2` and by `ListServiceStates` (called by a caller signing in `us-east-1`) as a change in `us-east-1`: two reads of one recorded change disagree, and the second attributes a change to a Region in which nothing was written.

The same divergence drops `EventName`: `entity_change_events` names the writing operation for the very same instants (`CreateServiceLevelObjective`/`UpdateServiceLevelObjective`), while `observed_service_states` passes `None`, so one recorded change is described two ways depending on which read reports it.

Fixing the Region leaves the existing test passing: `tests/all/list_service_states.rs` signs every request (creates and reads) with the same `eu-west-2` scope, so the record's Region and the caller's Region coincide there.

## Evidence

The documented rule, in this crate:
- `src/state.rs:3555` "/// `ChangeEvent.Region` is \"The Amazon Web Services region where this change event" / `:3556` "/// occurred\", and the change this event reports is the write of that record, so the Region" / `:3557` "/// is the record's own rather than the reader's."
- `src/state.rs:3567` "/// The ONE place this service builds a [`ChangeEvent`], so the two reads that report one —" / `:3568`-`:3572` naming `ListEntityEvents` … and `ListServiceStates` … "cannot differ on what an event of this emulator carries".

The side that follows it:
- `src/state.rs:3652` `change_event(at, attributes, record_region(arn), Some(event_name)),` (reached for `slo.created_time` at `:3666`-`:3671` and `slo.last_updated_time` at `:3675`-`:3680`).

The side that breaks it:
- `src/ops/list_service_states.rs:201` `// A change is reported in the caller's own region, read off the SigV4 credential` / `:202` `// scope exactly as a minted ARN's region is.` / `:203` `let region = crate::state::region_of(ctx.parts());`
- `src/state.rs:2073` `latest_change_events: vec![change_event(` / `:2074` `    changed_at,` / `:2076` `    region,` / `:2077` `    None,` — the caller's region, and no `EventName`, for instants read off the same SLO records (`src/state.rs:2021`-`:2029` `self.slos.values() … .flat_map(|slo| [slo.created_time, slo.last_updated_time])`).

Existing test (same region on every request, so it stays green after the fix):
- `tests/all/list_service_states.rs:26`-`:31` one `AUTHORIZATION` scope in `eu-west-2` for creates and reads; `:360` `assert_eq!(` / `:361` `    event["Region"].as_str(),` / `:362` `    Some(REGION),`.

## Steps to Reproduce

1. Create an SLO whose SliConfig names a service entity, signing the request with a credential scope in us-west-2 (so the stored ARN carries us-west-2).
2. Call ListEntityEvents for that entity over a window covering now, signing with a credential scope in us-east-1, and note ChangeEvents[].Region = us-west-2 (the record's region) and EventName = CreateServiceLevelObjective.
3. Call ListServiceStates over the same window, also signing in us-east-1.
4. Observe ServiceStates[].LatestChangeEvents[0].Region = us-east-1 (the reader's region) and no EventName — two reports of the same recorded change that disagree about where it occurred.
