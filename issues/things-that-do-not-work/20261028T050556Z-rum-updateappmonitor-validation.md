Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9dd770f9e2574e0600226bd25d044e51daa0c38

# UpdateAppMonitor accepts a deobfuscation configuration that enables source maps without the S3Uri the member documents as required

_Filed: 2026-10-28T05:05:56Z_
_Operation: UpdateAppMonitor | Class: validation_

> _Note: this issue was identified against the code base at commit `d9dd770f9e2574e0600226bd25d044e51daa0c38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The update path has the same unapplied rule as the create, and one extra consequence: because the member is replaced WHOLE ("the whole definition is replaced" semantics the handler states for the other members), an update naming `{"JavaScriptSourceMaps": {"Status": "ENABLED"}}` both skips the documented requirement and DROPS the `S3Uri` a previous create or update had stored. The monitor is left with `Status=ENABLED` and no bucket, `LastModified` is moved, and `GetAppMonitor` reports that state back. The model documents `JavaScriptSourceMaps$S3Uri` as "required if status is ENABLED", so the request should be refused with the `ValidationException` (HTTP 400) this operation declares, before the record is touched — which is exactly how the handler treats every other prose-only rule it checks two blocks above (the Domain/DomainList choice and the IncludedPages/ExcludedPages exclusion).

## Evidence

Model (smithy_query query="shape" shape="JavaScriptSourceMaps"), member `S3Uri`: "The S3Uri of the bucket or folder that stores the source map files. It is required if status is ENABLED."

The handler writes it unchecked — services/service-com-amazonaws-rum-rum/src/ops/update_app_monitor.rs:344-347:
```
        if let Some(deobfuscation) = input.deobfuscation_configuration {
            record.deobfuscation_configuration = deobfuscation;
            changed = true;
        }
```
while the SAME handler refuses the sibling prose-only rules before touching the record — update_app_monitor.rs:202-225:
```
        // The same prose-only rules `CreateAppMonitor` enforces: a monitor's domain
        // authority is held in exactly one of Domain/DomainList (this call may name
        // neither and leave the stored choice alone), and a configuration cannot carry
        // both IncludedPages and ExcludedPages.
        if let Some(message) = crate::state::domain_choice_refusal(
```
The crate has no S3Uri check at all (only src/ops/types/shared_types.rs:89, src/state.rs:1253, src/state.rs:742).

No test pins the current acceptance: the update cases always carry the pair — tests/all/app_monitor_lifecycle.rs:612-615 `json!({"DeobfuscationConfiguration": {"JavaScriptSourceMaps": {"Status": "ENABLED", "S3Uri": "s3://maps-bucket/app"}}})`.

## Steps to Reproduce

1. POST /appmonitor creating "maps-monitor" with DeobfuscationConfiguration {"JavaScriptSourceMaps":{"Status":"ENABLED","S3Uri":"s3://maps-bucket/app"}}
2. PATCH /appmonitor/maps-monitor with body {"DeobfuscationConfiguration":{"JavaScriptSourceMaps":{"Status":"ENABLED"}}}
3. Observe HTTP 200 instead of the modelled ValidationException (HTTP 400)
4. GET /appmonitor/maps-monitor: JavaScriptSourceMaps now reports Status=ENABLED with the previously stored S3Uri gone
