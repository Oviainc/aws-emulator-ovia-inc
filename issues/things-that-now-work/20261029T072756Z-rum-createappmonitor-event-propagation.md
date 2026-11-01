Issue-Severity: LOW
Issue-Gap-Class: event-propagation
Issue-Commit: ff66eaaa221f5d1d5d406f24a736c9617410f815

# A create refused under the write lock has already provisioned the vended CloudWatch Logs group, contradicting the handler's own ordering comment

_Filed: 2026-10-29T07:27:56Z_
_Operation: CreateAppMonitor | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `ff66eaaa221f5d1d5d406f24a736c9617410f815`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAppMonitor` provisions the monitor's vended CloudWatch Logs log group by calling the emulated CloudWatch Logs service (`logs:CreateLogGroup`) when the request carries `CwLogEnabled: true`. The handler's own comment states that this cross-service side effect is deliberately ordered AFTER the conflict and quota decisions so that "a create this service then refuses leaves no log group behind in CloudWatch Logs".

The code under that comment does not keep that promise. The conflict/quota decisions are taken TWICE: once under a short read lock (before provisioning) and again under the write lock (after provisioning), precisely because the handler holds no lock across the `.await`. The second set of refusals — duplicate name (`ConflictException`), per-region quota and region-bucket quota (`ServiceQuotaExceededException`) — returns after `provision_log_group` has already created `/aws/vendedlogs/RUMService_<name>` in CloudWatch Logs. So the one case the re-check exists for (a concurrent create that took the name or the last slot in between) is exactly the case where a refused create leaves a log group behind in another service's store — the outcome the comment says cannot happen.

The defect is the contradiction between the stated ordering rule and the code that implements it: either the re-checks must be taken before the group is provisioned, or the comment's claim is false. Note that `crate::state::cw_log_delivery`'s module docs admit only the OTHER orphan case ("a call whose future is dropped at that `.await` can leave the group behind"); the refused-create case is claimed as impossible rather than acknowledged.

## Evidence

The rule the handler states — `services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:310-320`:
```
        // Creating the monitor with CloudWatch Logs delivery already ON is the same
        // managed side effect `UpdateAppMonitor` performs when it turns delivery on: the
        // caller must hold `logs:PutResourcePolicy`, and the vended log group the
        // response's monitor will advertise has to exist.
        //
        // The PERMISSION is decided here, before anything is written. Provisioning the
        // group, though, is deferred until after the conflict and quota decisions below,
        // so a create this service then refuses leaves no log group behind in CloudWatch
        // Logs: the two refusals are read off the store, so they are taken under its lock
        // and the group is created in the gap between that lock and the insert.
        let enables_delivery = input.cw_log_enabled == Some(true);
```

The group is created here — `create_app_monitor.rs:374-401`:
```
        if enables_delivery {
            // The vended log group is created by CALLING the emulated CloudWatch Logs
            // service, over this emulator's OWN transport
...
            if let Err(detail) =
                crate::state::cw_log_delivery::provision_log_group(ctx, &region, &input.name)
                    .await
```

And the refusals that run AFTER it — `create_app_monitor.rs:404-427`:
```
        let mut store = state.lock();
        // Every refusal is taken again under the write lock: the checks above held no
        // lock across the provisioning step, so a concurrent create may have taken this
        // name, the region's last slot or the last region bucket in between.
        if store.contains_app_monitor(&region, &input.name) {
            return Err(CreateAppMonitorErrorModeled::ConflictException(
                crate::state::app_monitor_already_exists(&input.name),
            )
            .into());
        }
        if store.app_monitor_count(&region) >= crate::state::MAX_APP_MONITORS {
            return Err(CreateAppMonitorErrorModeled::ServiceQuotaExceededException(
```

The module that owns the side effect acknowledges only the dropped-future orphan, not this one — `services/service-com-amazonaws-rum-rum/src/state/cw_log_delivery.rs:37-40`:
```
//! * the group is created before the record is written, so a call whose future is dropped
//!   at that `.await` can leave the group behind. It is named after the app monitor
//!   ([`super::cw_log_group_name`]), so the next call that turns delivery on for the same
//!   monitor adopts that very group rather than colliding with it.
```

Test coverage: `services/service-com-amazonaws-rum-rum/tests/all/cw_log_delivery.rs` exercises only the ALLOWED create (the group is provisioned and advertised); no test drives a create that is refused under the write lock, so no existing assertion pins a log group being left behind.

## Steps to Reproduce

1. Mount RUM beside the emulated CloudWatch Logs service (as `tests/all/support.rs` does).
2. Create app monitor `m` with `POST /appmonitor {"Name":"m","Domain":"example.com"}` (delivery off), so the region already holds the name.
3. Read `create_app_monitor.rs:342-373`: the first conflict check is taken under a short lock and released; `create_app_monitor.rs:374-401` then provisions `/aws/vendedlogs/RUMService_m2`; `create_app_monitor.rs:404-413` re-takes the lock and answers `ConflictException` if the name is now taken.
4. Issue two concurrent `POST /appmonitor {"Name":"m2","Domain":"example.com","CwLogEnabled":true}` requests. One wins; the loser answers 409 ConflictException after its own `provision_log_group` call already succeeded.
5. Call CloudWatch Logs `DescribeLogGroups`: `/aws/vendedlogs/RUMService_m2` exists although the create that asked for it was refused — the outcome `create_app_monitor.rs:316-319` states cannot happen.
