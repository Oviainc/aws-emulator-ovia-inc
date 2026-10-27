Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# Most state-changing handlers leave no audit record, although PutAnomalyDetector's comment claims every mutating handler of the crate does

_Filed: 2026-10-26T08:25:28Z_
_Operation: CreateAnomalyDetector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::record_state_change` is the crate's one audit seam: "Record a call that CHANGED this service's state, so an emulator run is auditable the way CloudTrail makes a real one". Only 8 of the ~26 state-changing handlers call it. `PutAnomalyDetector` calls it on BOTH of its paths and documents the convention as a crate-wide invariant — "it leaves the audit record every other mutating handler of this crate leaves" — while `CreateAnomalyDetector`, which `PutAnomalyDetector`'s own comment says does "the same work" as its create path, never calls it.

The same split runs through the crate: `CreateScraper` records, `DeleteScraper` does not; `DeleteRuleGroupsNamespace` records, `CreateRuleGroupsNamespace` and `PutRuleGroupsNamespace` do not; `Delete/PutAlertManagerDefinition` and `DeleteLoggingConfiguration` record, while `CreateWorkspace`, `DeleteWorkspace`, `UpdateWorkspaceAlias`, `UpdateWorkspaceConfiguration`, `CreateAnomalyDetector`, `DeleteAnomalyDetector`, `CreateLoggingConfiguration`, `UpdateLoggingConfiguration`, `Create/Update/DeleteQueryLoggingConfiguration`, `DeleteScraperLoggingConfiguration`, `PutResourcePolicy`, `DeleteResourcePolicy`, `TagResource` and `UntagResource` all mutate the store and record nothing (several of them discard the request parts outright with `let _ = ctx;`).

So the audit trail an operator reads is silently partial, and the stated invariant is false: whether a mutation is auditable depends on which handler happened to be written with the call.

## Evidence

The helper and the rule it states — `src/state.rs:1319-1327`:
```
/// Record a call that CHANGED this service's state, so an emulator run is auditable the
/// way CloudTrail makes a real one: the operation, the resource it changed and the
/// access-key id the REQUEST presented.
...
pub fn record_state_change(operation: &str, resource: &str, parts: &::http::request::Parts) {
```

The claim — `src/ops/put_anomaly_detector.rs:336-341`:
```
            // The call CHANGED this service's state, so it leaves the audit record every
            // other mutating handler of this crate leaves — request metadata only (see
            // [`crate::state::record_state_change`]).
            let response = response_for(detector);
            crate::state::record_state_change(Self::NAME, &response.arn, ctx.parts());
```
and on its create path — `src/ops/put_anomaly_detector.rs:344-346, 409-410`:
```
        // The CREATE path: ... It is the same work `CreateAnomalyDetector`
        // does, so it is bound by the same two rules that operation applies
...
        // A create changes this service's state too, so it leaves the same audit record.
        crate::state::record_state_change(Self::NAME, &response.arn, ctx.parts());
```

The contradicting sibling — `src/ops/create_anomaly_detector.rs:343-351` persists the detector and returns with no audit call at all:
```
        if let Some(token) = input.client_token {
            workspace
                .anomaly_detector_tokens
                .insert(token, anomaly_detector_id.clone());
        }
        workspace
            .anomaly_detectors
            .insert(anomaly_detector_id, detector);
        Ok(response)
```
(`grep record_state_change services/service-com-amazonaws-amp-amazonprometheusservice/src` returns matches only in `state.rs` and in create_alert_manager_definition, put_alert_manager_definition, create_scraper, update_scraper, put_anomaly_detector, delete_rule_groups_namespace, delete_logging_configuration, delete_alert_manager_definition.)

Further inconsistency inside one resource family — `src/ops/create_scraper.rs:337-340` records the create, while `src/ops/delete_scraper.rs:154` throws the request parts away: `let _ = ctx;` and never records the deletion it persists at `src/ops/delete_scraper.rs:209-216`.

Test coverage: no test asserts on the audit record either way, so adding the missing calls changes no response and no existing test.

## Steps to Reproduce

1. Run the emulator with tracing at info level.
2. PutAnomalyDetector on a workspace that holds no detector with that id: an "aps state change" record is emitted (src/ops/put_anomaly_detector.rs:410).
3. CreateAnomalyDetector in the same workspace: a detector is persisted and NO record is emitted, although put_anomaly_detector.rs:344-346 states the two paths do "the same work" and put_anomaly_detector.rs:336-338 states every mutating handler of this crate leaves the record.
4. Repeat with CreateWorkspace, DeleteWorkspace, DeleteScraper, TagResource, UntagResource, PutResourcePolicy: each persists a change and emits nothing, while CreateScraper, UpdateScraper, DeleteRuleGroupsNamespace, DeleteLoggingConfiguration and both alert-manager mutators do emit.
