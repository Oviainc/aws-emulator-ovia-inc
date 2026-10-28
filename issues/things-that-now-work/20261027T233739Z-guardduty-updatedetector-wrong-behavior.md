Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# An UpdateDetector that carries no configuration member still moves the detector's UpdatedAt, breaking the rule its two siblings state for the same field

_Filed: 2026-10-27T23:37:39Z_
_Operation: UpdateDetector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every member of `UpdateDetectorRequest` except the `DetectorId` URI label is OPTIONAL (`Enable`, `FindingPublishingFrequency`, `DataSources`, `Features`), so `POST /detector/{DetectorId}` with the body `{}` is a well-formed request that asks for no change at all. The handler applies each optional member only when it is present (and both `apply_data_sources` / `apply_feature_configurations` return immediately on `None`), but then writes `detector.updated_at_millis = now` UNCONDITIONALLY, so a request that changed nothing still moves the `UpdatedAt` that `GetDetector` reports.

This contradicts the rule the same crate states and applies for the SAME field on the SAME record in its two sibling detector-configuration writes:

* `UpdateMalwareScanSettings` — whose members are also all optional — tracks a `changed` flag and gates the write on it, with a comment that spells the rule out.
* `UpdateOrganizationConfiguration` writes it unconditionally and explicitly JUSTIFIES that by the fact that one of its two auto-enablement views is required, so "an accepted request always writes the auto-enablement". That justification does not hold for `UpdateDetector`, which can accept a request that writes nothing.

The effect is an observable timestamp on the detector that advances with no change behind it: a client polling `GetDetector` to detect configuration drift sees the detector as freshly modified after an empty update, and the crate's own `UpdateMalwareScanSettings` comment says that is exactly what must not happen ("a request that carried no setting changed nothing, so it moves no timestamp either").

## Evidence

Model — all configurable members optional (`smithy_query(operation="UpdateDetector")`): `"Enable": {"target": "Boolean", "jsonName": "enable", "type": "boolean"}, "FindingPublishingFrequency": {... "type": "enum"}, "DataSources": {...}, "Features": {... "type": "list"}` — none carries `required`, only `DetectorId` does.

The ungated write — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/update_detector.rs:188-202:
```
        if let Some(enable) = input.enable {
            detector.status = if enable { ... } else { ... };
        }
        if let Some(frequency) = input.finding_publishing_frequency.clone() {
            detector.finding_publishing_frequency = Some(frequency);
        }
        // The deprecated `DataSources` view is applied first so an explicit
        // `Features` entry for the same switch wins.
        crate::state::apply_data_sources(detector, input.data_sources.as_ref(), now);
        crate::state::apply_feature_configurations(detector, input.features.as_ref(), now);
        detector.updated_at_millis = now;
```
Both appliers no-op on an absent member — src/state.rs:5666 `let Some(features) = features else { return };` and src/state.rs:5937-5943 `apply_data_sources` -> `apply_data_sources_to(..., data_sources, now)` with the same early return — so with an empty body line 202 is the ONLY write.

Sibling that states and applies the opposite rule — src/ops/update_malware_scan_settings.rs:187-207:
```
        let mut changed = false;
        ...
        // `UpdatedAt` is "the last-updated timestamp for the detector"
        // (`smithy_query(operation="GetDetector")`) and the scan settings are part of
        // the detector's configuration — but a request that carried no setting
        // changed nothing, so it moves no timestamp either.
        if changed {
            detector.updated_at_millis = now;
        }
```

Sibling whose unconditional write is justified by a REQUIRED member — src/ops/update_organization_configuration.rs:235-242:
```
        // `UpdatedAt` is "the last-updated timestamp for the detector"
        // (`smithy_query(operation="GetDetector")`) ... so a write that lands here moves the
        // timestamp a read reports. Unconditional: exactly one of the two
        // auto-enablement views is required (refused above otherwise), so an
        // accepted request always writes the auto-enablement.
        detector.updated_at_millis = now;
```

Test coverage: tests/all/detector_configuration_timestamp.rs pins only writes that DO carry a value — `CONFIGURATION_WRITES` (lines 50-109) has nine rows, every one of them with a non-empty body (e.g. `("the detector's own status", "", || json!({"enable": false}))`), plus `a refused write must not move updatedAt` (line 274) and `a read must not move updatedAt` (line 330). No row sends an empty `UpdateDetector` body, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateDetector in the request Region and note the returned detectorId.
2. GET /detector/{detectorId} and record `updatedAt` (call it T0).
3. POST /detector/{detectorId} with the body `{}` — a well-formed UpdateDetector that names no configuration member. It answers 200.
4. GET /detector/{detectorId} again and read `updatedAt`.
5. Expected, per the rule src/ops/update_malware_scan_settings.rs:201-207 states for the same field: `updatedAt` is still T0, because nothing changed. Actual: `updatedAt` has advanced past T0 (src/ops/update_detector.rs:202 wrote it unconditionally), while GetDetector reports the identical status, publishing frequency and feature set as before.
6. For contrast, repeat with POST /detector/{detectorId}/malware-scan-settings and body `{}`: that sibling answers 200 and leaves `updatedAt` at T0.
