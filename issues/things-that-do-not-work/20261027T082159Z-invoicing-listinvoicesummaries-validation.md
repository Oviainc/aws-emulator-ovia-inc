Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# ListInvoiceSummaries accepts a ReceiverRole filter over a period the model documents as a validation error

_Filed: 2026-10-27T08:21:59Z_
_Operation: ListInvoiceSummaries | Class: validation_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents three temporal rules that hang off `InvoiceSummariesFilter$ReceiverRole`: a query whose period precedes 2025-06-01 must be refused when `ReceiverRole` is specified, a `TimeInterval` with `ReceiverRole` is bounded at five years, and a `TimeInterval` without `ReceiverRole` is bounded at one month. The handler implements none of them.

`SummaryFilter::new` is the only place the request's `Filter` is inspected before the walk, and the only thing it refuses is a `ReceiverRole` value the enum does not declare; the `TimeInterval` is merely converted into two instant keys and then used as an inclusive/exclusive comparison. `ListInvoiceSummaries`'s handler has exactly three refusal paths — an invalid `NextToken`, an unmodelled `selector.resourceType`, and an unmodelled `filter.receiverRole` — so a request stating `ReceiverRole` together with a `TimeInterval` that starts in 2024, or an unbounded/over-long interval, is answered 200 instead of the documented `ValidationException`.

Nothing in the crate (no comment, constant or doc) mentions the 2025-06-01 floor or either interval bound, so the divergence is unacknowledged rather than deliberate.

## Evidence

Smithy, `smithy_query(query="doc", shape="InvoiceSummariesFilter", member="ReceiverRole")`:
  "When ReceiverRole is specified: … Data is available starting 2025-06-01. Queries for periods before 2025-06-01 return a validation error. … TimeInterval supports a time interval of up to 5 years. Without ReceiverRole, TimeInterval is limited to one month."

Handler — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_summaries.rs:258-281 (the whole of `SummaryFilter::new`):
```
        let Some(filter) = filter else {
            return Ok(Self::default());
        };
        if let Some(role) = filter.receiver_role.as_deref() {
            modelled_enum_value("filter.receiverRole", role, RECEIVER_ROLE_VALUES)?;
        }
        Ok(Self {
            time_interval: filter.time_interval.as_ref().map(|interval| {
                (
                    crate::state::instant_key(&interval.start_date),
                    crate::state::instant_key(&interval.end_date),
                )
            }),
```
— the interval is converted and never checked, and the only later use is the comparison at :291-296 (`issued >= start && issued < end`). No `ValidationException` in this file is built for a `TimeInterval`.

Crate-wide: `grep -n '2025-06-01|5 years|one month' services/service-com-amazonaws-invoicing-invoicing/**/*.rs` matches nothing, so no comment or test states a reason for the divergence.

Test coverage today pins the opposite and would need its dates moved: services/service-com-amazonaws-invoicing-invoicing/tests/all/invoice_summaries.rs:33 `const JAN_15: i64 = 1_705_276_800;` (2024-01-15) and :376-384
```
            json!({
                "ReceiverRole": "BUYER",
                "TimeInterval": {"StartDate": JAN_15, "EndDate": MAR_16},
```
which is asserted through `ok(&router, &body)` at :389 — a 200 for a `ReceiverRole` query over a 2024 period.

## Steps to Reproduce

1. Start the emulator and seed/own an account with invoices (or simply call the operation — the refusal must precede the registry read).
2. POST / with `X-Amz-Target: Invoicing.ListInvoiceSummaries` and body `{"Selector":{"ResourceType":"ACCOUNT_ID","Value":"<account>"},"Filter":{"ReceiverRole":"BUYER","TimeInterval":{"StartDate":1705276800,"EndDate":1710547200}}}` (2024-01-15 → 2024-03-16).
3. Observe HTTP 200 with an `InvoiceSummaries` list; the model documents that a query for a period before 2025-06-01 with `ReceiverRole` specified returns a validation error.
4. Repeat with no `ReceiverRole` and a `TimeInterval` spanning two months (`StartDate` 1705276800, `EndDate` 1710547200): observe HTTP 200, though the model bounds an interval without `ReceiverRole` at one month.
