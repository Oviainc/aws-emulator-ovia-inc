Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: ab8b9af343344bd9ec1c9404b5c1438075827ba1

# The CloudWatch-Logs-delivery permission refusal is worded two ways: CreateAppMonitor answers the generic IAM sentence where the sibling UpdateAppMonitor answers RUM's own

_Filed: 2026-10-28T21:41:20Z_
_Operation: CreateAppMonitor | Class: error-response_

> _Note: this issue was identified against the code base at commit `ab8b9af343344bd9ec1c9404b5c1438075827ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Turning `CwLogEnabled` on is one managed side effect with one cross-service precondition, and both handlers that can turn it on run the SAME check through the SAME helper (`crate::state::cw_log_delivery::caller_may_enable_delivery`). `CreateAppMonitor`'s own comment says so in as many words. But the two handlers render that one refusal differently:

* `UpdateAppMonitor` answers `AccessDeniedException` with RUM's own sentence, built from the request's `Name` member (`"Error updating the application <name>"`), and a test pins it with a stated reason.
* `CreateAppMonitor` throws the `Denied` through the crate's generic projection `crate::state::access_denied`, so the same client mistake on the create path gets the IAM-style sentence `"User: <principal> is not authorized to perform: logs:PutResourcePolicy on resource: arn:aws:logs:<region>:<account>:log-group:/aws/vendedlogs/RUMService_<name> because no policy allows the logs:PutResourcePolicy action"`.

So one condition, decided by one helper, is reported in two vocabularies by two sibling handlers of the same crate — and the one path the crate has evidence for (the update, test-pinned) is not the path that uses the generic wording. A client that branches on, logs or asserts the message of this refusal sees a different body depending on whether delivery was turned on at create time or afterwards. Nothing in the crate states a reason for the difference; the create path's message is pinned by no test.

## Evidence

`CreateAppMonitor` says the two paths are the same side effect, then words the refusal generically — `services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:310-333`:
```
        // Creating the monitor with CloudWatch Logs delivery already ON is the same
        // managed side effect `UpdateAppMonitor` performs when it turns delivery on: the
        // caller must hold `logs:PutResourcePolicy`, and the vended log group the
        // response's monitor will advertise has to exist.
...
        let enables_delivery = input.cw_log_enabled == Some(true);
        if enables_delivery {
            if let Err(denial) = crate::state::cw_log_delivery::caller_may_enable_delivery(
                &*self.engine,
                ctx,
                &region,
                &input.name,
            ) {
                return Err(CreateAppMonitorErrorModeled::AccessDeniedException(
                    crate::state::access_denied(&denial),
                )
                .into());
            }
        }
```

The sibling, same helper, different words — `services/service-com-amazonaws-rum-rum/src/ops/update_app_monitor.rs:280-297`:
```
        if input.cw_log_enabled == Some(true) {
            if crate::state::cw_log_delivery::caller_may_enable_delivery(
                &*self.engine,
                ctx,
                &region,
                &input.name,
            )
            .is_err()
            {
                // The wording AWS answers this refusal with, built from the request's
                // `Name` member.
                return Err(UpdateAppMonitorErrorModeled::AccessDeniedException(
                    AccessDeniedException {
                        message: format!("Error updating the application {}", input.name),
                    },
                )
                .into());
            }
```

What the create path's message actually is — `services/service-com-amazonaws-rum-rum/src/state.rs:1919-1923` and `src/state/authorization.rs:180-190`:
```
pub fn access_denied(denial: &authorization::Denied) -> AccessDeniedException {
    AccessDeniedException {
        message: denial.message(),
    }
}
```
```
        format!(
            "User: {} is not authorized to perform: {} on resource: {} {reason}",
            self.principal, self.action, self.resource
        )
```

The one check both call — `services/service-com-amazonaws-rum-rum/src/state/cw_log_delivery.rs:133-146`:
```
pub fn caller_may_enable_delivery<Eng: Send + Sync + 'static>(
...
    authorization::authorize_identity(
        engine,
        ctx,
        authorization::LOGS_PUT_RESOURCE_POLICY,
        &super::cw_log_group_arn(region, &log_group),
    )
```

Test coverage: only the UPDATE path is covered. `services/service-com-amazonaws-rum-rum/tests/all/authorization.rs:1144-1237` drives every row of `enabling_cw_log_delivery_requires_the_callers_logs_put_resource_policy` through `PATCH /appmonitor/{monitor}` with `{"CwLogEnabled": enable}` and asserts
```
            // The wording AWS answers this refusal with is built from the request's
            // `Name` member.
            assert_eq!(
                reply.message(case),
                format!("Error updating the application {monitor}"),
```
No test sends a `CreateAppMonitor` with `CwLogEnabled: true` from a caller lacking `logs:PutResourcePolicy` (`tests/all/cw_log_delivery.rs` only exercises the allowed create, lines 63-124), so nothing pins the create path's generic sentence and aligning it changes no existing assertion.

## Steps to Reproduce

1. Create an assumed-role caller whose inline policy allows `rum:*` on `*` and nothing in the `logs` namespace (the first row of `tests/all/authorization.rs:1148-1153`).
2. As that caller, send `PATCH /appmonitor/m {"CwLogEnabled": true}` for an existing monitor `m`. Observed: 403 `AccessDeniedException` with message `Error updating the application m`.
3. As the same caller, send `POST /appmonitor {"Name": "m2", "Domain": "example.com", "CwLogEnabled": true}`. Observed: 403 `AccessDeniedException` with the generic `User: arn:aws:sts::...:assumed-role/... is not authorized to perform: logs:PutResourcePolicy on resource: arn:aws:logs:us-east-1:...:log-group:/aws/vendedlogs/RUMService_m2 because no policy allows the logs:PutResourcePolicy action`.
4. Compare: one condition, one helper, two different response bodies from two sibling handlers of this crate, with only the update side's wording backed by anything in the crate.
