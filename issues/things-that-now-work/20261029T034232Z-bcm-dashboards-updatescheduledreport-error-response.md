Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e79dd1ec68971fa3f627b273166ce9e704773849

# The date/time-range refusal quotes DateTimeValue$value whole, so ValidationException.message runs past its modelled 1024-character bound

_Filed: 2026-10-29T03:42:32Z_
_Operation: UpdateScheduledReport | Class: error-response_

> _Note: this issue was identified against the code base at commit `e79dd1ec68971fa3f627b273166ce9e704773849`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every write that submits a `DateTimeRange` refuses a malformed bound through the shared helper `crate::state::date_time_range_refusal`, whose message is built by `malformed_date_time_value_message` and quotes the submitted `value` VERBATIM. `DateTimeValue$value` targets `GenericString` (`@length min 0, max 1024`), and `ValidationException$message` targets the SAME shape with the same 1024 maximum — so quoting a maximal value plus the ~175 characters of fixed clause and member path puts a `message` of roughly 1200 characters on the wire, past the bound the model declares for that member.

The crate already owns the rule and the helper that enforces it: `MAX_MESSAGE_LENGTH`/`within_message_bound` exist precisely so "a message built by quoting them is bounded HERE ... before it goes on the wire", and sibling refusal builders in this same crate route through it (`check_tag_keys_not_reserved`, `ResourceNotFound::new`, and `CreateScheduledReport`'s dashboard-not-found message, whose comment argues the rule explicitly). The four handlers that call `date_time_range_refusal` lift its string straight into the modelled `ValidationException` without that bound:

* `UpdateScheduledReport` and `CreateScheduledReport` for `widgetDateRangeOverride` (path `widgetDateRangeOverride.startTime.value: '<1024 chars>' ...`);
* `CreateDashboard` and `UpdateDashboard` for every widget query's `timeRange` (via `widget_list_refusal`), where the member path is longer still, e.g. `widgets[1].configs[1].queryParameters.costAndUsage.timeRange.startTime.value`.

The fix belongs in the one helper (bound the built message the way the reserved-tag-key refusal now is); it is filed against `UpdateScheduledReport` as the shortest reproducing caller — the overflowing member sits at the request's top level there.

## Evidence

Model — the two bounds that collide, `smithy_query(query="shape",shape="DateTimeValue,ValidationException,GenericString")`:
> `"DateTimeValue": {... "value": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^[\\S\\s]*$"...}}`
> `"ValidationException": {... "members": {"message": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^[\\S\\s]*$"}}}`

The unbounded quote — `src/state.rs:1817-1828` (`malformed_date_time_value_message`):
```rust
    let requirement = if point.r#type == DATE_TIME_TYPE_ABSOLUTE {
        "the value of an ABSOLUTE date/time value must be an absolute date, for example \
         '2025-07-01'"
...
    format!(
        "{path}.value: '{}' is not a valid {} date/time value; {requirement}",
        point.value, point.r#type
    )
```
reached from `src/state.rs:1854-1859`:
```rust
        if !names_a_point_in_time(point) {
            return Some(malformed_date_time_value_message(
                &format!("{path}.{member}"),
                point,
            ));
```

The caller that puts it on the wire untouched — `src/ops/update_scheduled_report.rs:243-250`:
```rust
        if let Some(message) = input.widget_date_range_override.as_ref().and_then(|range| {
            crate::state::date_time_range_refusal("widgetDateRangeOverride", range)
        }) {
...
            return Err(UpdateScheduledReportErrorModeled::ValidationException(
                ValidationException { message },
```
(the identical lift is `src/ops/create_scheduled_report.rs:244-250` and, via `widget_list_refusal`, `src/ops/create_dashboard.rs:209-214` and `src/ops/update_dashboard.rs:204-211`).

The rule this crate states for itself — `src/state.rs:156-165`:
> "Every error shape on this surface declares its `message` against `GenericString`, whose `@length` is `min 0, max 1024` ... The members a message quotes are bounded far more loosely than that ... so a message built by quoting them is bounded HERE, by [`within_message_bound`], before it goes on the wire. A client that sized or stored a message against the maximum its own copy of the model declares then never sees a value the model said could not occur."

A sibling refusal on the SAME handler family that does apply it — `src/ops/create_scheduled_report.rs:313-324`:
> "// `ResourceNotFoundException`, so the refusal is its modelled // `ValidationException` (400), with the offending ARN in the message — quoted // within the bound that member declares (`crate::state::within_message_bound`), // because `DashboardArn` admits twice the text `ValidationException$message` does."
```rust
                    message: crate::state::within_message_bound(format!(
                        "Dashboard not found: {}",
                        requested.dashboard_arn
                    )),
```

Test coverage: `tests/all/error_message_length_bound.rs:5-9` states the invariant — "a message is a MODELLED member, so whatever a refusal quotes from the request is quoted within the `@length` maximum that member's target shape declares — 1024 characters" — but its rows only drive reserved tag keys and ARN templates (`key_rows()`, `arn_lengths()`); no row submits a `DateTimeValue$value` at all. `tests/all/widget_time_range_validation.rs:113-137,463-488` asserts the exact message for SHORT values (`absolute_form_message`/`relative_form_message` built from 1-to-20-character values), so bounding the built message leaves every one of those rows passing.

## Steps to Reproduce

1. CreateDashboard with `{"name": "Bound Probe", "widgets": []}` and keep the returned `arn`.
2. CreateScheduledReport with `{"scheduledReport": {"name": "Bound Probe Report", "dashboardArn": <dashboard arn>, "scheduledReportExecutionRoleArn": "arn:aws:iam::123456789012:role/r", "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}}}` and keep the report `arn`.
3. Call UpdateScheduledReport with `{"arn": <report arn>, "widgetDateRangeOverride": {"startTime": {"type": "ABSOLUTE", "value": "x".repeat(1024)}, "endTime": {"type": "ABSOLUTE", "value": "2025-07-01"}}}` — the 1024-character value satisfies `GenericString`'s `@length 0..1024` and `^[\S\s]*$`, so it reaches the handler.
4. Expect HTTP 400 `ValidationException` whose `message` honours the modelled `ValidationException$message` maximum of 1024 characters (the way the reserved-tag-key refusal now does).
5. Observe a `message` of roughly 1200 characters: `widgetDateRangeOverride.startTime.value: '<the whole 1024-character value>' is not a valid ABSOLUTE date/time value; the value of an ABSOLUTE date/time value must be an absolute date, for example '2025-07-01'`.
6. Repeat through CreateDashboard with one widget whose `configs[0].queryParameters.costAndUsage.timeRange.startTime` carries the same 1024-character value — the same helper produces an even longer body, since the widget member path adds ~60 characters.
