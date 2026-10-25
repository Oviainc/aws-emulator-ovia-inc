Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 11e424e87637980d781b5ebada45988cd752adfb

# PutRumMetricsDestination stores an Evidently IamRoleArn without the documented PassRole permission check

_Filed: 2026-10-25T01:00:19Z_
_Operation: PutRumMetricsDestination | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `11e424e87637980d781b5ebada45988cd752adfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An Evidently destination makes RUM assume the `IamRoleArn` the request names, and the model documents the caller-side precondition for passing that role: "If you specify this parameter, you must be signed on to a role that has PassRole permissions attached to it, to allow the role to be passed." The handler validates only the member PAIRING rules (`metric_destination_choice_refusal`, `metric_destination_role_refusal`), authorizes `rum:PutRumMetricsDestination` on the app monitor, and then stores the role ARN — so any caller allowed the RUM action alone can hand RUM an arbitrary role ARN and have it persisted and reported by `ListRumMetricsDestinations`.

This crate already implements exactly this kind of precondition for its other managed side effect: when `CreateAppMonitor`/`UpdateAppMonitor` turn `CwLogEnabled` on, `cw_log_delivery::caller_may_enable_delivery` makes an identity-only decision for the documented cross-service permission (`logs:PutResourcePolicy`) against the foreign resource's ARN, BEFORE anything is written, and refuses with `AccessDeniedException`. The role-passing path needing the same treatment calls no such helper, so the two managed side effects of one service are guarded inconsistently.

## Evidence

Model (smithy_query doc, `PutRumMetricsDestinationRequest$IamRoleArn`): "This parameter is required if Destination is Evidently. ... This parameter specifies the ARN of an IAM role that RUM will assume to write to the Evidently experiment that you are sending metrics to. ... If you specify this parameter, you must be signed on to a role that has PassRole permissions attached to it, to allow the role to be passed. The CloudWatchAmazonCloudWatchRUMFullAccess policy doesn't include PassRole permissions."

Handler, services/service-com-amazonaws-rum-rum/src/ops/put_rum_metrics_destination.rs:193-203 (the only checks made on the role) and :250-277:
```
let record = crate::state::MetricDestinationRecord {
    destination: input.destination.clone(),
    destination_arn: destination_arn.map(str::to_string),
    iam_role_arn: iam_role_arn.map(str::to_string),
};
...
monitor.metric_destinations.insert(key, record);
```
The single authorization call in the file is for the RUM action only, put_rum_metrics_destination.rs:222-229: `crate::state::authorization::authorize(..., crate::state::authorization::PUT_RUM_METRICS_DESTINATION, &resource_arn, held_policy.as_deref(), &[])`.

Sibling guard in the same crate, services/service-com-amazonaws-rum-rum/src/state/cw_log_delivery.rs:11-17:
```
//! 1. **the caller must hold `logs:PutResourcePolicy`.** RUM copies the monitor's
//!    telemetry into a log group in the caller's own account, which means installing a
//!    resource policy on that log group on the caller's behalf, so a caller that may not
//!    install one may not turn delivery on either. The check is an identity-based
//!    decision in the CloudWatch Logs namespace ... and it is made BEFORE anything is
//!    written
```
implemented at cw_log_delivery.rs:82-95 through the existing `authorization::authorize_identity(engine, ctx, ACTION, &arn)` helper, and called from create_app_monitor.rs:266-278 and update_app_monitor.rs:248-265. `PutRumMetricsDestination` makes no such call.

Test coverage: tests/all/authorization.rs tables the per-operation `rum:` action decisions (e.g. authorization.rs:771-801) and tests/all/metrics_destinations.rs exercises put/list/delete; none of them grants or withholds a role-passing permission, so no existing test pins the current acceptance.

## Steps to Reproduce

1. Create an app monitor and grant the caller only `rum:PutRumMetricsDestination` on it (no role-passing permission).
2. POST /rummetrics/{name}/metricsdestination with {"Destination": "Evidently", "DestinationArn": "arn:aws:evidently:us-east-1:<account>:experiment/exp", "IamRoleArn": "arn:aws:iam::<account>:role/not-passable"}.
3. Observe HTTP 200 and the role stored: GET /rummetrics/{name}/metricsdestination reports the `IamRoleArn`.
4. Contrast with the CwLog path: PATCH /appmonitor/{name} with {"CwLogEnabled": true} for a caller lacking `logs:PutResourcePolicy` is refused with AccessDeniedException before anything is written.
