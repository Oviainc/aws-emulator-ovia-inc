Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: fad1fdecc61c47880f0bf921597b8afa67a7a57d

# The shared positional cursor honours nextToken spellings the service never hands out ("+1", "007"), serving a page for a token that continues nothing

_Filed: 2026-10-29T18:25:01Z_
_Operation: ListAlerts | Class: pagination_

> _Note: this issue was identified against the code base at commit `fad1fdecc61c47880f0bf921597b8afa67a7a57d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListPosition::read` is this service's one positional-cursor rule, shared by `ListAlerts`, `ListClusterAlerts` and `DescribeSchedule`. Its documentation states the rule it enforces: a token "that is not a whole number is NOT read as the first page: it continues nothing this service handed out, so it is refused."

The implementation refuses by `str::parse::<usize>()`, which accepts spellings this service never hands out: a leading plus (`"+1"`) and leading zeros (`"007"`, `"0"` padded, etc.). The hand-out side is `reported.to_string()` on a `usize`, which only ever produces a bare, unpadded decimal, so `"+1"` and `"007"` continue no page this service ever issued — yet they are served as the page at position 1 and 7 instead of being refused with the listing's modelled 400.

The sibling listing that pages by a catalog id is strict on exactly this point: `ListOfferings` refuses any `nextToken` that is not one of the ids "this operation hands a page's token out as". The positional listings accept forms they never hand out.

## Evidence

The rule, stated by the shared cursor — services/service-com-amazonaws-medialive-medialive/src/state.rs:1236-1252:
```
    /// A request naming no token asks for the first page. A token that is not a
    /// whole number is NOT read as the first page: it continues nothing this
    /// service handed out, so it is refused. A position PAST the end is not
    /// refused, because these sequences shrink on their own: ...
    pub fn read(
        next_token: Option<&str>,
        page_size: usize,
        position_of: &str,
    ) -> Result<Self, String> {
        let already_reported = match next_token {
            Some(token) => token
                .parse::<usize>()
                .map_err(|_| format!("nextToken {token} is not {position_of} position"))?,
            None => 0,
        };
```
`"+1"` and `"007"` both parse as `usize`, so both are admitted.

The only form the service hands out — src/state.rs:1279-1281:
```
        let reported = self.already_reported.saturating_add(page.len());
        let next_token = (reported < sequence.len()).then(|| reported.to_string());
        (page, next_token)
```
(`usize::to_string` never emits a sign or leading zeros.)

The call site named by this issue — src/ops/list_alerts.rs:197-202:
```
        let cursor = crate::state::ListPosition::read(
            input.next_token.as_deref(),
            page_size,
            "an alert",
        )
        .map_err(|message| bad_request(&message))?;
```
(the same `read` is called from src/ops/list_cluster_alerts.rs:192 and src/ops/describe_schedule.rs:175-180).

The sibling that IS strict about "a token this operation hands out" — src/ops/list_offerings.rs:210-222:
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

Test coverage, and why a fix is additive: tests/all/shared_position_cursor_rule.rs:316-330 enumerates the tokens that must be refused —
```
const NOT_POSITIONS: [&str; 9] = [
    "halfway",
    "-1",
    "1.5",
    "1e2",
    "0x1",
    " 1",
    "1 ",
    "1,2",
    "99999999999999999999999999999999",
];
```
— neither `"+1"` nor `"007"` is among them, and the test's own rule statement (lines 332-337: "a token that is not a whole number is refused with the modelled 400") is satisfied by a stricter digits-only read, so tightening `read` keeps every existing row green.

## Steps to Reproduce

1. Create a channel and seed two alerts on it (tests/all/channel_alerts.rs shows the seam `ChannelRecord::raise_alert`), so the channel's alert log has at least two entries.
2. GET /prod/channels/{channelId}/alerts?maxResults=1 -> 200 with one alert and `nextToken` = "1" — the only spelling this service hands out (src/state.rs:1280).
3. GET /prod/channels/{channelId}/alerts?maxResults=1&nextToken=%2B1 ("+1") -> 200, served as the page at position 1, although the service never handed out "+1".
4. GET /prod/channels/{channelId}/alerts?maxResults=1&nextToken=007 -> 200 with the page at position 7 (an empty last page for a short log), although the service never handed out "007".
5. Compare with src/state.rs:1236-1239 ("it continues nothing this service handed out, so it is refused") and with the refusal for "halfway"/"-1"/" 1" that the same path does produce; the same two spellings are accepted by ListClusterAlerts and DescribeSchedule, which share `ListPosition::read`.
