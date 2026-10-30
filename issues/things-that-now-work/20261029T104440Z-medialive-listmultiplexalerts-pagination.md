Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: c8784923130dd4420e009c5117a8bbfb746443f0

# nextToken is honoured in spellings this operation never hands out ("+7000001", "07000001"), so a token the service never issued is served as a page

_Filed: 2026-10-29T10:44:40Z_
_Operation: ListMultiplexAlerts | Class: pagination_

> _Note: this issue was identified against the code base at commit `c8784923130dd4420e009c5117a8bbfb746443f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListMultiplexAlerts` states that its `NextToken` is the `Id` of the alert the next page starts at and that "a value that is not an alert id at all cannot be one of its tokens and is a bad request rather than an empty page". It then decodes the token with `next_token.parse::<u64>()`, which accepts spellings the minting site never writes: Rust's unsigned integer parser admits an optional leading `+` and arbitrary leading zeros, so `?nextToken=%2B7000001` and `?nextToken=07000001` are both read as the alert id 7000001 and served as a page, although the only thing this operation ever hands out is the plain decimal rendering of the stored `u64` (`alert.id.to_string()`).

The same decode is copied into the three other position/id readers of this crate, so the same tokens are honoured there too: `ListAlerts` (`token.parse::<usize>()`), `ListClusterAlerts` (`token.parse::<usize>()`) and `DescribeSchedule` (`token.parse::<usize>()`), each under a comment claiming that a value that is not a position/id is refused.

The sibling listing that implements the stated rule is `ListOfferings`: it resolves the token through `crate::state::offering_index` and refuses anything that is not one of the ids it hands out, with the message "nextToken must be the offeringId this operation hands a page's token out as". The alert/schedule readers word the same rule and then do not enforce it.

This is not about the tolerance these listings deliberately grant a token whose row has since been evicted (an id/position past the end resumes at the end — pinned by tests with a stated reason). It is about a token string the service can never have minted being accepted instead of refused.

## Evidence

The rule this operation states for its own token — services/service-com-amazonaws-medialive-medialive/src/ops/list_multiplex_alerts.rs:186-201:
```
        // `NextToken` is the `Id` of the alert the next page starts at, which is
        // the form this operation hands out, so a value that is not an alert id
        // at all cannot be one of its tokens and is a bad request rather than an
        // empty page. ...
        let first = match input.next_token.as_deref() {
            None => None,
            Some(next_token) => Some(next_token.parse::<u64>().map_err(|_| {
                bad_request(format!(
                    "nextToken must be the alert id this operation hands out a page's token as, \
                     got {next_token:?}"
                ))
            })?),
        };
```

The ONE minting site, which only ever writes a plain decimal — src/state/multiplexes.rs:1192-1194:
```
        // Absent on the last page: a token is handed out only when an alert the
        // same filter admits is left over after the ones this page carries.
        let next_token = remaining.next().map(|alert| alert.id.to_string());
```
over an id that is a `u64` — src/state/multiplexes.rs:198-200:
```
    /// The minted id, "The unique ID for this alert instance", which is also the
    /// key this alert is stored at and the `NextToken` a page resumes from.
    id: u64,
```

The sibling that enforces the same rule — src/ops/list_offerings.rs:210-222:
```
        // `NextToken` is the `OfferingId` the next page starts at, which is the
        // form this operation hands a token out as, so a value that is not one
        // of this catalog's offering ids cannot be one of its tokens and is a
        // bad request rather than an empty page.
        let start = match input.next_token.as_deref() {
            None => 0,
            Some(next_token) => crate::state::offering_index(next_token).ok_or_else(|| {
                bad_request(format!(
                    "nextToken must be the offeringId this operation hands a page's token out \
                     as, got {next_token:?}"
                ))
            })?,
        };
```

The same lenient decode, copied — src/ops/list_alerts.rs:200-205:
```
        let already_reported = match input.next_token.as_deref() {
            Some(token) => token
                .parse::<usize>()
                .map_err(|_| bad_request(&format!("nextToken {token} is not an alert position")))?,
            None => 0,
        };
```
src/ops/list_cluster_alerts.rs:194-199 and src/ops/describe_schedule.rs:179-184 ("nextToken {token} is not a schedule position") are the same three lines again, each over a token minted as `reported.to_string()` (src/ops/describe_schedule.rs:209, src/ops/list_alerts.rs:232).

Test coverage: the refusal rows stop short of the non-canonical spellings — tests/all/multiplex_alerts.rs:941-957:
```
    // A `NextToken` that is not the id form this operation hands a token out as
    // is a bad request; the one it did hand out resumes the walk. A numeric
    // token no alert has is a RESUME POINT past that id, not a refusal, which is
    // what lets a page resume after an alert the bounded store has evicted.
    for next_token in ["abc", "7000001x", "-1", "1.5", ""] {
```
and the deliberate tolerance is pinned separately at tests/all/multiplex_alerts.rs:958-979 ("0", "9999999999"). The sibling copies are pinned the same partial way: tests/all/cluster_alerts.rs:727-739 (rows "a token that is not a position" => `?nextToken=later`, "a negative position" => `?nextToken=-1`) and tests/all/channel_schedule.rs:322-327 (rows `nextToken=halfway`, `nextToken=1.5`). `ListOfferings`' strict reading IS pinned — tests/all/offering_catalog.rs:745-747 ("nextToken=not-a-token", "nextToken=0", "nextToken=99999999" all refused).

## Steps to Reproduce

1. Create a multiplex and a program on it, and let the clock settle so the multiplex raises an alert (as tests/all/multiplex_alerts.rs sets up).
2. GET /prod/multiplexes/{MultiplexId}/alerts?maxResults=1 -> 200 with a nextToken, e.g. "7000001".
3. GET /prod/multiplexes/{MultiplexId}/alerts?nextToken=%2B7000001 (a leading '+') -> the emulator answers 200 and serves the page starting at alert 7000001. Expected, per the handler's own rule at src/ops/list_multiplex_alerts.rs:186-189 and the sibling src/ops/list_offerings.rs:210-222, a 400 BadRequestException naming the value, because "+7000001" is not a token this operation hands out (the only minting site writes `alert.id.to_string()`, src/state/multiplexes.rs:1194).
4. GET /prod/multiplexes/{MultiplexId}/alerts?nextToken=07000001 -> 200, same page, for the same reason.
5. Repeat against the three sibling readers for the same acceptance: GET /prod/channels/{ChannelId}/alerts?nextToken=%2B1, GET /prod/clusters/{ClusterId}/alerts?nextToken=01, GET /prod/channels/{ChannelId}/schedule?nextToken=%2B1 — each answers 200 while the same handler refuses "later"/"1.5" with 400 (tests/all/cluster_alerts.rs:733-738, tests/all/channel_schedule.rs:325-326).
