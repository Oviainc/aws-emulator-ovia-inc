Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9dd770f9e2574e0600226bd25d044e51daa0c38

# A DeobfuscationConfiguration with JavaScriptSourceMaps.Status=ENABLED and no S3Uri is accepted and stored

_Filed: 2026-10-28T05:05:56Z_
_Operation: CreateAppMonitor | Class: validation_

> _Note: this issue was identified against the code base at commit `d9dd770f9e2574e0600226bd25d044e51daa0c38`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAppMonitor` applies every OTHER prose-only member-pairing rule of this service before it writes — the Domain/DomainList choice, the IncludedPages/ExcludedPages exclusion — but it never applies the one the model states for the deobfuscation configuration: `JavaScriptSourceMaps$S3Uri` is documented as required when `Status` is `ENABLED`. The handler stores whatever `DeobfuscationConfiguration` the request carried verbatim, so a create naming `{"JavaScriptSourceMaps": {"Status": "ENABLED"}}` with no `S3Uri` is answered 200 and the monitor is persisted with source-map deobfuscation turned ON and no bucket to read the maps from. `GetAppMonitor` then reports that configuration back (`to_app_monitor` projects the stored record unchanged), so the emulator both accepts and advertises a configuration the model declares invalid instead of answering the `ValidationException` (HTTP 400) the operation declares. The boundary validator cannot catch it: `S3Uri` is an OPTIONAL member with only a `@length` and a `@pattern`, so an absent value satisfies every modelled constraint and the conditional requirement is prose the handler owns.

## Evidence

Model (smithy_query query="shape" shape="JavaScriptSourceMaps"), member `S3Uri`: "The S3Uri of the bucket or folder that stores the source map files. It is required if status is ENABLED." — and `S3Uri` is not `@required`, only `length {min:1,max:1024}` + `@pattern`, while `Status` is `@required`.

The handler stores it unchecked — services/service-com-amazonaws-rum-rum/src/ops/create_app_monitor.rs:419-421:
```
            deobfuscation_configuration: input
                .deobfuscation_configuration
                .unwrap_or_else(crate::state::default_deobfuscation_configuration),
```
No check for it exists anywhere in the crate: `grep -n 's3_uri|S3Uri' services/service-com-amazonaws-rum-rum/src` finds only the field declaration (src/ops/types/shared_types.rs:89), the default (src/state.rs:1253 `s3_uri: None`) and the projection (src/state.rs:742).

The same handler DOES refuse the sibling prose-only rules — create_app_monitor.rs:240-249:
```
        if let Some(message) = input
            .app_monitor_configuration
            .as_ref()
            .and_then(crate::state::app_monitor_configuration_refusal)
```
whose helper states exactly this contract — src/state.rs:1307-1313: "From the model: `AppMonitorConfiguration$IncludedPages` and `$ExcludedPages` both document \"You can't include both ExcludedPages and IncludedPages in the same operation\", a rule no modelled constraint expresses, so every operation that accepts a configuration refuses it here with the `ValidationException` it declares." The identical shape of rule is also enforced for the metrics destinations (src/state.rs:1366-1380 `metric_destination_role_refusal`, quoting "This parameter is required if Destination is Evidently").

No test pins the current acceptance: every deobfuscation case in the suite sends `S3Uri` beside the status — tests/all/app_monitor_lifecycle.rs:87-88 `"DeobfuscationConfiguration": {"JavaScriptSourceMaps": {"Status": deobfuscation, "S3Uri": "s3://maps-bucket/app"}}` (both DISABLED and ENABLED, lines 41/79) and :612-615, and tests/all/constraint_refusals.rs covers Name/Domain/DomainList/enum refusals only.

## Steps to Reproduce

1. POST /appmonitor with body {"Name":"maps-monitor","Domain":"example.com","DeobfuscationConfiguration":{"JavaScriptSourceMaps":{"Status":"ENABLED"}}} (no S3Uri)
2. Observe HTTP 200 with a minted Id instead of the modelled ValidationException (HTTP 400)
3. GET /appmonitor/maps-monitor and observe AppMonitor.DeobfuscationConfiguration.JavaScriptSourceMaps = {"Status":"ENABLED"} with no S3Uri — deobfuscation enabled with no source-map bucket
4. Compare with the same request carrying both IncludedPages and ExcludedPages, which IS refused 400 by crate::state::app_monitor_configuration_refusal
