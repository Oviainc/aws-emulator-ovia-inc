Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: de2d8767cb82ce68bb0f28dff69738907970fd37

# The positional nextToken rule is implemented three times in this crate's handlers instead of once beside the page-size rule it sits next to

_Filed: 2026-10-29T15:39:54Z_
_Operation: ListAlerts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `de2d8767cb82ce68bb0f28dff69738907970fd37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three handlers paginate by POSITION in a derived sequence (ListAlerts, ListClusterAlerts, DescribeSchedule). Each one hand-rolls the same rule inline: parse `nextToken` as a whole-number position or refuse with the operation's 400, `skip(already_reported).take(page_size)`, `reported = already_reported.saturating_add(len)`, and hand out `reported.to_string()` only when `reported < total`. ListAlerts and ListClusterAlerts are character-for-character identical, down to the message text.

That is one rule in three copies, in a crate that centralises exactly this kind of listing rule: all three handlers call `crate::state::list_page_size`, whose doc says "ONE copy of this page-size rule and of its wording … so none of them can drift into its own reading of the floor, of the ceiling or of what an absent member asks for", and the alert `StateFilter` membership rule is shared the same way (`crate::state::alert_state_filter`). The positional-cursor rule, which decides which page a token resumes at and when a token is handed out at all, is the one listing rule left per-handler, so the three copies can drift (and a defect in it has to be found and fixed three times).

The fix is a shared `crate::state` helper taking the position noun for the message, leaving every response byte and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-medialive-medialive/src/ops/list_alerts.rs:200-205 and :222-232:
```
        let already_reported = match input.next_token.as_deref() {
            Some(token) => token
                .parse::<usize>()
                .map_err(|_| bad_request(&format!("nextToken {token} is not an alert position")))?,
            None => 0,
        };
...
        let reportable = channel.alerts.in_set_order(state_filter);
        let alerts: Vec<ChannelAlert> = reportable
            .iter()
            .skip(already_reported)
            .take(page_size)
            .map(|record| crate::state::channel_alert_output(record))
            .collect();
        let reported = already_reported.saturating_add(alerts.len());
        let next_token = (reported < reportable.len()).then(|| reported.to_string());
```

Copy 2 — src/ops/list_cluster_alerts.rs:194-199 and :226-235 (same rule, same message string):
```
        let already_reported = match input.next_token.as_deref() {
            Some(token) => token.parse::<usize>().map_err(|_| {
                bad_request(&format!("nextToken {token} is not an alert position"))
            })?,
            None => 0,
        };
...
        let alerts: Vec<ClusterAlert> = admitted
            .iter()
            .skip(already_reported)
            .take(page_size)
            .map(|alert| crate::state::cluster_alert_output(alert))
            .collect();
        let reported = already_reported.saturating_add(alerts.len());
        let next_token = (reported < admitted.len()).then(|| reported.to_string());
```

Copy 3 — src/ops/describe_schedule.rs:179-184 and :199-209:
```
        let already_reported = match input.next_token.as_deref() {
            Some(token) => token.parse::<usize>().map_err(|_| {
                bad_request(&format!("nextToken {token} is not a schedule position"))
            })?,
            None => 0,
        };
...
        let reported = already_reported.saturating_add(schedule_actions.len());
        let next_token = (reported < ordered.len()).then(|| reported.to_string());
```

The crate's stated convention for exactly this kind of rule — src/state.rs:1169-1182:
```
/// ONE copy of this page-size rule and of its wording, for the reason
/// [`bounded_length`] states: every paginated operation of this service cuts its
/// page by the same rule, so none of them can drift into its own reading of the
/// floor, of the ceiling or of what an absent member asks for.
...
pub fn list_page_size(max_results: Option<i32>) -> Result<usize, String> {
```
No positional-cursor helper exists in `src/state` (no `position` helper there other than the schedule's internal `start_position`).

Existing tests that pin the behaviour and must stay green unchanged: tests/all/channel_alerts.rs:697-724 and :873 (ListAlerts positional tokens), tests/all/cluster_alerts.rs:733 ("a token that is not a position") and :786-797, tests/all/channel_schedule.rs:269-285 and :325-361 ("a token that is not a position", "a token that is not a whole number", a position past the end), plus tests/all/shared_page_size_rule.rs and tests/all/list_page_size.rs for the rule that already is shared.

## Steps to Reproduce

1. Open src/ops/list_alerts.rs and src/ops/list_cluster_alerts.rs side by side: lines 200-205 and 194-199 are the same parse-or-refuse branch with the same message literal, and lines 222-232 / 226-235 are the same skip/take/`saturating_add`/`then(|| reported.to_string())` computation.
2. Open src/ops/describe_schedule.rs:179-184 and :199-209 for the third copy of the same rule (only the message noun differs).
3. Compare with src/state.rs:1182 `list_page_size`, the sibling listing rule all three handlers call from one place, whose doc states the one-copy convention.
4. Grep `src/state` for a positional-cursor helper: none exists, so there is no single place the rule could drift from.
