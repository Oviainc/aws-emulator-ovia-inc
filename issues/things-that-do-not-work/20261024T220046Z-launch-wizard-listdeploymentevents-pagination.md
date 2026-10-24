Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# ListDeploymentEvents pagination cursor is a mutable index into a log that is not append-only, so a resumed page silently skips events

_Filed: 2026-10-24T22:00:46Z_
_Operation: ListDeploymentEvents | Class: pagination_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDeploymentEvents` builds each page token out of the event's POSITION in `Deployment::events` (`format!("{}:{position}", deployment.id)` over `.enumerate()`), and `crate::state::paginate` resumes by finding the item whose cursor equals the decoded one and starting at `index + 1`. The handler justifies this with the claim that the history is append-only, so a position is a stable identity.

That premise is false inside this same crate: `Deployment::append_event` — the ONE path every appender goes through — drains the OLDEST events once the log passes `MAX_EVENTS_PER_DEPLOYMENT`. When that trimming happens between two pages of a walk, every surviving event's index shifts down by the number dropped, so the saved `nextToken` (`"<id>:<pos>"`) now resolves to a DIFFERENT, newer event. The second page therefore resumes further along the history than it should and the events in between are never returned to the caller — and because the log is kept exactly at the cap, the cursor always still matches an index, so no `ValidationException` ("NextToken is not valid.") is raised either: the caller gets a successful, silently incomplete walk.

The defect is the choice of a mutable array index as the cursor identity in a log the crate itself rewrites, not the value of the cap: the same cap with a stable per-event cursor (e.g. one derived from data that trimming does not renumber) would page correctly.

## Evidence

Handler's cursor and its stated premise — `services/service-com-amazonaws-launchwizard-launchwizard/src/ops/list_deployment_events.rs:170-184`:
```
// Events are reported oldest first, in the order the lifecycle appended them;
// an event has no id of its own, so a page resumes after its position in that
// append-only history. The deployment id is folded into the cursor, so a token
// one deployment's event list issued is not accepted on another's.
let events: Vec<(String, DeploymentEventDataSummary)> = deployment
    .events
    .iter()
    .enumerate()
    .map(|(position, event)| {
        (
            format!("{}:{position}", deployment.id),
            event.to_summary(),
        )
    })
```

The same crate contradicts "append-only" — `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:649-661`:
```
/// Append `event` to this deployment's history, dropping the OLDEST events once
/// the log is over [`MAX_EVENTS_PER_DEPLOYMENT`].
...
pub fn append_event(&mut self, event: DeploymentEvent) {
    self.events.push(event);
    if self.events.len() > MAX_EVENTS_PER_DEPLOYMENT {
        let overflow = self.events.len() - MAX_EVENTS_PER_DEPLOYMENT;
        self.events.drain(..overflow);
    }
}
```

How the token is consumed — `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2129-2134`:
```
let cursor =
    decode_token(token).ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?;
let found = items.iter().position(|item| cursor_of(item) == cursor);
found
    .map(|index| index + 1)
    .ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?
```

Test coverage: `services/service-com-amazonaws-launchwizard-launchwizard/tests/all/deployment_update.rs:945-951` proves the trimming is reachable and observable over the wire ("The log is capped at `state::MAX_EVENTS_PER_DEPLOYMENT`, and a capped log drops its OLDEST events"), but its `walk_pages(..., "/listDeploymentEvents", ..., 100)` calls at lines 975-983 and 1016-1024 page over a log that is NOT mutated between pages, so the interleaving of a page walk with trimming is untested.

## Steps to Reproduce

1. Create a deployment (`POST /createDeployment`) against a seeded workload/pattern and let the clock-owed steps settle, so the record has several events.
2. Call `POST /listDeploymentEvents` with `{"deploymentId": "<id>", "maxResults": 1}` and keep the returned `nextToken` (it encodes the cursor `"<id>:0"`).
3. Grow that deployment's event log past `MAX_EVENTS_PER_DEPLOYMENT` (200): repeat `POST /updateDeployment` on the same deployment, waiting for each update to finish, since every update appends two events (the route `tests/all/deployment_update.rs:952-973` short-circuits by calling `Deployment::append_event` directly for exactly this reason).
4. Call `POST /listDeploymentEvents` again with `{"deploymentId": "<id>", "maxResults": 1, "nextToken": "<saved token>"}`.
5. Observe a 200 whose page resumes after whatever event now sits at index 0 — i.e. the walk jumps forward by the number of trimmed events and never reports the ones in between, with no `ValidationException` to tell the caller the token no longer names the position it was issued for.
