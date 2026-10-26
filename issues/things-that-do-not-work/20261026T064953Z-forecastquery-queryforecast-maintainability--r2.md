Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# The documented date LAYOUT walk is implemented twice in state.rs, so the two copies can drift

_Filed: 2026-10-26T06:49:53Z_
_Operation: QueryForecast | Class: maintainability_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both query handlers judge `StartDate`/`EndDate` through two separate hand-rolled walks of the SAME documented layout — `yyyy-MM-dd` + a date/time separator + `HH:mm[:ss]` — stated once in `crate::state::parse_date_time` (the instant a date names, used by the projection and by the point clip) and a second time in `crate::state::matches_date_time_pattern` (the shape-layer pattern check run by both handlers as their first post-ARN step). Each one independently encodes the same positional rule: ASCII digits everywhere except `-` at index 4 and 7, the date/time separator at index 10, and `:` at index 13. The two differ only in how far they go (exact 19 bytes with `:` at index 16 vs. a 16-byte prefix plus a trailing character class) and in whether a lowercase `t` is admitted, so the shared positions are duplicated logic: a change to how this crate reads the documented layout — a separator, a field width, the ASCII-only decision — must be made in both or the two layers silently disagree about which values are dates.

The fix is to state the positional walk once (parameterised by the two documented differences) and have both functions call it. No response changes: every status, code and message stays byte-identical, since the composite behaviour of each function is unchanged.

## Evidence

Copy 1 — `parse_date_time`, services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/state.rs:682-699:
```rust
pub fn parse_date_time(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let separator = match index {
            4 | 7 => Some(b'-'),
            10 => Some(b'T'),
            13 | 16 => Some(b':'),
            _ => None,
        };
        match separator {
            Some(expected) if *byte != expected => return None,
            None if !byte.is_ascii_digit() => return None,
            _ => {}
        }
    }
```

Copy 2 — `matches_date_time_pattern`, src/state.rs:773-796:
```rust
fn matches_date_time_pattern(value: &str) -> bool {
    /// Length of the fixed layout the pattern opens with, up to and including the minutes.
    const LAYOUT: usize = 16;

    let bytes = value.as_bytes();
    if bytes.len() < LAYOUT {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate().take(LAYOUT) {
        let satisfied = match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T' || *byte == b't',
            13 => *byte == b':',
            _ => byte.is_ascii_digit(),
        };
        if !satisfied {
            return false;
        }
    }
```

Both are reached from the same handler, within a few lines of each other: `crate::state::shape_validation_message(&input.filters, input.start_date.as_deref(), input.end_date.as_deref())` at src/ops/query_forecast.rs:222-226 (which calls `matches_date_time_pattern` via src/state.rs:917-923) and `crate::state::FORECAST.resolve_query(..)` at src/ops/query_forecast.rs:243-256 (which reaches `parse_date_time` via `DateWindow::parse`, src/state.rs:940-950, and via `DateWindow::clip`, src/state.rs:965-977). The identical pair is reached from src/ops/query_what_if_forecast.rs:227-233 and 250-263.

That both copies restate ONE documented layout is the crate's own claim — src/state.rs:764-769: "Read position by position rather than through a regex engine, for the same reason [`parse_date_time`] is: the pattern is a FIXED LAYOUT — `yyyy-MM-dd`, then `T` or `t`, then `HH:mm` — followed by one character class, so walking it IS the pattern, and the place this crate already judges a date does it exactly this way."

Test coverage that pins both layers and must stay green unchanged: tests/all/shape_validation.rs:205 `a_date_outside_the_shape_pattern_is_refused_before_the_arn_is_resolved` (9 rejected and 6 accepted spellings, both operations, both members) and tests/all/forecastquery.rs:555 `a_date_outside_the_documented_format_is_refused` (8 rejected values with the exact expected code per layer, plus the accepted leap day).

## Steps to Reproduce

1. Open services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/state.rs and compare `parse_date_time` (lines 682-699) with `matches_date_time_pattern` (lines 773-796): both encode `-` at index 4 and 7, the date/time separator at index 10 and `:` at index 13, with ASCII digits elsewhere.
2. Change the layout rule in ONE of them only (for example admit a lowercase `t` in `parse_date_time`, or require a zero-padded field differently) and observe that the shape layer and the projection now disagree about which values are dates, while nothing in the crate ties the two walks together.
3. Run `cargo test -p service-com-amazonaws-forecastquery-amazonforecastruntime` and note that tests/all/shape_validation.rs::a_date_outside_the_shape_pattern_is_refused_before_the_arn_is_resolved exercises only copy 2 and tests/all/forecastquery.rs::a_date_outside_the_documented_format_is_refused exercises both, so a drift inside the shared positions is only caught indirectly.
