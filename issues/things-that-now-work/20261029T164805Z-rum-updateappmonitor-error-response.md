Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 94313aadf7366efaf8ae51cba58459f4f1ec87e1

# A request that breaks a boundary-applied constraint AND a handler-applied pattern is stated with the boundary's clauses alone, dropping the handler's clause the crate's own rule 1 promises

_Filed: 2026-10-29T16:48:05Z_
_Operation: UpdateAppMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `94313aadf7366efaf8ae51cba58459f4f1ec87e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate takes responsibility for the modelled constraints the framework's boundary walk leaves unapplied (`AppMonitorName`, `AppMonitorDomain`, `TagKey`, `DimensionName` — the look-around `@pattern`s its regex engine cannot compile, plus the map-key constraints the walk skips), and it states a constraint refusal under one documented rule: "the envelope is AWS's `N validation error(s) detected: ` over one clause per failing (member, requirement)", holding "for every constrained member of every operation".

That rule is broken whenever ONE request breaks both kinds of constraint at once. `crate::service`'s gate collects the framework's violations and refuses the request IN FRONT of the operation, so the handler — the only place the look-around patterns are ever applied — never runs, and its clause is silently absent from the envelope. The response then reports `1 validation error(s) detected` where the request actually broke two (member, requirement) pairs, and a caller that fixes the one clause it was told about is refused a second time on the clause the first answer withheld.

`UpdateAppMonitor` is one such operation: its handler is where `AppMonitorDomain`'s pattern is applied to `Domain` and to every element of `DomainList` (`crate::state::validation::app_monitor_refusal`), while `AppMonitorConfiguration$SessionSampleRate` carries a modelled `@range(min: 0, max: 1)` and `AppMonitorDomainList` a modelled `@length(min: 1, max: 5)` that the boundary walk does apply. A request breaking one of each is answered with the boundary half only.

The gap is admitted in the crate ('KNOWN LIMIT, documented rather than papered over'), and nothing in the crate tests it: the only test of the multi-clause envelope uses two HANDLER-side pattern violations, where the rule does hold.

## Evidence

The rule the crate states for every constrained member of every operation — `services/service-com-amazonaws-rum-rum/src/state/validation.rs:52-56`:
```
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report cited:
//!
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `, in the order the walk found them;
```

The same module admitting the rule is broken — `services/service-com-amazonaws-rum-rum/src/state/validation.rs:37-42`:
```
//! KNOWN LIMIT, documented rather than papered over: the boundary walk runs BEFORE a
//! handler and `crate::service` states its violations in front of the operation, so a
//! request that breaks a boundary-applied constraint AND one of the patterns here is stated
//! with the boundary's clauses alone — the handler it would have been refused by never
//! runs. Closing it needs the core seam that hands a handler the input the walk already
//! built, which `services/common/src/validation.rs` records.
```

The gate that refuses in front of the operation — `services/service-com-amazonaws-rum-rum/src/service.rs:286-290` (and `AnnouncedMediaType::stated_refusal`, `src/service.rs:413`):
```
//! owner of the request — its telemetry included. The exceptions are the routes the
//! protocol's routing key cannot name ([`QUERY_UNMATCHED_ROUTES`]), which the gate serves
//! from the operations it holds, and a request the framework's own constraint walk refuses,
//! which the gate states in front of the operation because that is the only place the
//! request's own values are still in hand ([`AnnouncedMediaType::stated_refusal`]).
```

The handler clause that is lost — `services/service-com-amazonaws-rum-rum/src/ops/update_app_monitor.rs:184-200`:
```
        // The modelled `@pattern` of `AppMonitorDomain`, which `Domain` targets directly
        // and `DomainList`'s member targets too: written with look-around, so the boundary
        // validator admits any value for it and this call is where it is applied — before
        // the stored record is touched ...
        if let Some(message) = crate::state::validation::app_monitor_refusal(
            None,
            input.domain.as_deref(),
            input.domain_list.as_deref(),
        ) {
```

The two constraint kinds that can collide in one `UpdateAppMonitor` request, from the model (`smithy_query(query="shape", shape="AppMonitorConfiguration,AppMonitorDomainList")`): `SessionSampleRate` carries `"range": {"min": 0, "max": 1}` (boundary-applied), while `AppMonitorDomainList`'s member targets `AppMonitorDomain`, whose `@pattern` is `^(localhost)$|...(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)...` — the look-around expression `services/service-com-amazonaws-rum-rum/src/state/validation.rs:184` transcribes because the walk cannot compile it.

Test coverage: the only test of the multi-clause envelope breaks two HANDLER-applied patterns, so it never exercises the mixed case — `services/service-com-amazonaws-rum-rum/tests/all/constraint_refusals.rs:345-358`:
```
/// RULE: a request that breaks TWO members is two validation errors, stated in one
/// envelope in the order the input declares them — not the first one found.
#[tokio::test]
async fn two_offending_members_are_two_validation_errors_in_one_envelope() {
    ...
    let reply = create_call(&router, json!({"Name": ".bad", "Domain": "ex..ample.com"})).await;
    let expected = format!(
        "2 validation errors detected: ...
```
No test in the crate pins the mixed boundary+handler case, so no existing expectation states a reason for the current single-clause answer.

## Steps to Reproduce

1. CreateAppMonitor(Name="mixed-clauses", Domain="example.com") — 200
2. PATCH /appmonitor/mixed-clauses with body {"DomainList": ["bad_domain"], "AppMonitorConfiguration": {"SessionSampleRate": 5}} — the request breaks two (member, requirement) pairs: `sessionSampleRate` against its modelled @range(0,1) (boundary-applied) and `domainList`'s element against AppMonitorDomain's @pattern (handler-applied; `_` is outside the pattern's alphabet)
3. Observed: HTTP 400 ValidationException whose message is `1 validation error(s) detected: ...sessionSampleRate...` — the domainList pattern clause is absent, because the gate refused the request before update_app_monitor.rs:191 ran
4. Expected per the crate's own rule 1 ("one clause per failing (member, requirement)"): `2 validation error(s) detected: ` with both the sessionSampleRate clause and the domainList pattern clause
5. Fix the sample rate and resend {"DomainList": ["bad_domain"], "AppMonitorConfiguration": {"SessionSampleRate": 0.5}} — now refused on the domain clause the first answer withheld, so one invalid request costs two round trips
