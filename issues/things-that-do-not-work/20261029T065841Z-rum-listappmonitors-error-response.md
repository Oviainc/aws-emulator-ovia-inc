Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 5d001f011ed28660c188b78afabc22e4b6c273f2

# ListAppMonitors makes the authorization decision before it judges NextToken, so a token it never issued is answered 403 where every sibling listing answers the modelled 400

_Filed: 2026-10-29T06:58:41Z_
_Operation: ListAppMonitors | Class: error-response_

> _Note: this issue was identified against the code base at commit `5d001f011ed28660c188b78afabc22e4b6c273f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has one stated rule for the order a handler judges a request in: a refusal about the request's OWN members — a check that reads neither the store nor the caller's identity — is answered BEFORE the resource not-found and BEFORE the authorization decision, because the framework's boundary walk runs in front of everything a handler judges. `ListAppMonitors` inverts it: `authorize_identity` runs at list_app_monitors.rs:157-167, and only afterwards, at list_app_monitors.rs:169-181, is `NextToken` decoded and a token the service never issued refused with the modelled `ValidationException`.

The three sibling paginated readers do it the other way round, each with the decode ahead of the decision, and `GetAppMonitorData`'s equivalent own-member check (its `Filters` key check) is a row in the crate's own precedence table. The consequence: a caller holding no `rum:ListAppMonitors` grant that passes a malformed or foreign `nextToken` is answered `AccessDeniedException` (403), while the identical client mistake on `ListRumMetricsDestinations`, `BatchGetRumMetricDefinitions` or `GetAppMonitorData` is answered `ValidationException` (400) for that same caller. One rule, four listings, two answers.

This is the same defect class a FIXED issue already corrected for a sibling: `20261029T050526Z-rum-putresourcepolicy-error-response` ("PutResourcePolicy judges PolicyDocument's size and syntax only after resolving the monitor and authorizing, unlike every sibling handler in the crate"); that fix is what put the rule-stating comment at put_resource_policy.rs:185-189 and the member-check rows in the precedence suite. `ListAppMonitors`' token check was left behind.

## Evidence

The rule, as this crate states it — `services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs:185-189`:
```
        // policy is "not formatted correctly". Judged FIRST, like every other handler's
        // checks on the request's OWN members, because they stand in for constraints the
        // boundary walk cannot apply and that walk runs in front of everything a handler
        // judges — neither check reads the store or the caller's identity, so neither
        // waits on the resolution below or on the authorization decision after it.
```
and `services/service-com-amazonaws-rum-rum/tests/all/member_check_precedence.rs:6-12`:
```
//! The rule under test: a refusal that is about the request's own members — a check that
//! stands in for a constraint the framework's boundary walk cannot apply, and that reads
//! neither the store nor the caller's identity — is answered BEFORE the not-found for the
//! resource the request addresses and before the authorization decision, because that
//! walk runs in front of everything a handler judges.
```
(that table's last row, member_check_precedence.rs:241-252, is exactly a token/filter-class own-member check: "GetAppMonitorData, a Filters entry naming a field it does not search" → 400 `ValidationException` ahead of both the 404 and the 403.)

The offending order — `services/service-com-amazonaws-rum-rum/src/ops/list_app_monitors.rs:157-181`:
```
        if let Err(denial) = crate::state::authorization::authorize_identity(
            &*self.engine,
            ctx,
            crate::state::authorization::LIST_APP_MONITORS,
            &crate::state::app_monitor_collection_arn(&region),
        ) {
            return Err(ListAppMonitorsErrorModeled::AccessDeniedException(
                crate::state::access_denied(&denial),
            )
            .into());
        }

        // No token starts at the beginning; a token resumes strictly after the last
        // summary the page that produced it returned. A token this operation never
        // issued is refused rather than read as some other resume point.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(decode_next_token(token).ok_or_else(|| {
                ListAppMonitorsErrorModeled::ValidationException(ValidationException {
```

The siblings, token first and authorization second:
- `services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:168-183` (decode + `ValidationException`) then `:207-221` (`authorize`);
- `services/service-com-amazonaws-rum-rum/src/ops/batch_get_rum_metric_definitions.rs:196-217` (decode) then `:241-255` (`authorize`);
- `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor_data.rs:172-183` (decode) and `:188-202` (filter keys) then `:219-231` (`authorize`).

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/app_monitor_lifecycle.rs:1009-1068` (`list_app_monitors_refuses_a_token_it_never_issued`) drives every bogus-token row through the account's own credential, which is always allowed, so it pins the 400 only for an authorized caller and passes either way; `services/service-com-amazonaws-rum-rum/tests/all/authorization.rs:778-783` drives the `rum:ListAppMonitors` denial row with `body: None` and no `nextToken`, so it pins the 403 only for a request carrying no token. Nothing pins the current order.

## Steps to Reproduce

1. Create an IAM role with no policies and assume it through STS, so the caller holds no `rum:ListAppMonitors` grant (the `assumed_role_key("holds-no-grant", None)` helper the crate's own suites use).
2. Call POST /appmonitors?maxResults=1&nextToken=not-hex-at-all signed with that credential. Observed: HTTP 403 `AccessDeniedException`. Expected: HTTP 400 `ValidationException` ("NextToken not-hex-at-all is not a pagination token this service issued."), the answer the crate's precedence rule and its three sibling listings give.
3. Call GET /rummetrics/{AppMonitorName}/metricsdestination?nextToken=not-hex-at-all with the same unauthorized credential against an existing monitor: HTTP 400 `ValidationException` — the sibling answer the listing above should match.
4. Call POST /appmonitor/{Name}/data with the same credential and body {"TimeRange":{"After":0},"NextToken":"not-hex-at-all"}: HTTP 400 `ValidationException`, pinned for the filter-key form of the same check by tests/all/member_check_precedence.rs.
