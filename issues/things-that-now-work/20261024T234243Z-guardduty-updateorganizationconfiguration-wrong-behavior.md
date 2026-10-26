Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# UpdateOrganizationConfiguration writes the detector's organization configuration without moving the detector's UpdatedAt, unlike the sibling configuration write

_Filed: 2026-10-24T23:42:43Z_
_Operation: UpdateOrganizationConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateOrganizationConfiguration` mutates a field of the stored `Detector` record (`detector.organization` — the auto-enable setting and the per-feature organization switches) and never touches `detector.updated_at_millis`. The sibling write to the OTHER configuration field of the very same record, `UpdateMalwareScanSettings`, moves that timestamp and states the rule it follows while doing so. The result is that `GetDetector` reports the same `UpdatedAt` before and after an organization configuration change, while it reports a moved `UpdatedAt` after a malware-scan-settings change — two writes to two fields of one record, with two different answers about whether the detector changed.

The omission is also SILENT: every other handler in this crate that deliberately leaves the detector's `UpdatedAt` alone says so in a comment and gives its reason (the sub-resource handlers: "the set of sub-resources it holds is unchanged, and only this IPSet's record changed"). `UpdateOrganizationConfiguration` neither moves the timestamp nor states why it does not, and the word `updated_at` does not appear in the file at all.

## Evidence

The sibling write moves it, and quotes the rule it is following — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/ops/update_malware_scan_settings.rs:201-207:
```
        // `UpdatedAt` is "the last-updated timestamp for the detector"
        // (`smithy_query(operation="GetDetector")`) and the scan settings are part of
        // the detector's configuration — but a request that carried no setting
        // changed nothing, so it moves no timestamp either.
        if changed {
            detector.updated_at_millis = now;
        }
```
This handler writes the detector's own record and moves nothing — src/ops/update_organization_configuration.rs:203-231:
```
        let configuration = &mut detector.organization;
...
        // The deprecated `DataSources` view is applied first so an explicit
        // `Features` entry for the same switch wins.
        crate::state::apply_org_data_sources(configuration, input.data_sources.as_ref());
        crate::state::apply_org_feature_configurations(configuration, input.features.as_ref());
```
`grep -n 'updated_at|UpdatedAt' src/ops/update_organization_configuration.rs` → "No matches." (and neither `apply_org_data_sources` nor `apply_org_feature_configurations`, src/state.rs:5615 and 5664, takes a `now` argument, so no timestamp moves inside them either).

The configuration is part of the detector record, not a sub-resource with its own identity — src/state.rs:1996-2002:
```
    /// What this detector's account has configured for the OTHER accounts in its
    /// organization — what `DescribeOrganizationConfiguration` reports and
    /// `UpdateOrganizationConfiguration` writes.
    ///
    /// It hangs off the detector because that is what identifies it on the wire
    /// (`/detector/{DetectorId}/admin`), so `DeleteDetector` drops it with the
    /// detector.
```
Every handler that intentionally leaves the detector's timestamp alone states that choice — src/ops/update_ip_set.rs:215-218:
```
        // `Unit` output: success is the modelled 200 and an empty body. The
        // detector's own `UpdatedAt` does not move: the set of sub-resources it
        // holds is unchanged, and only this IPSet's record changed.
```
(also src/ops/delete_filter.rs:162-166 and src/ops/update_publishing_destination.rs:230-233).

Test coverage: the rule is pinned for the sibling configuration write — tests/all/malware_scan_settings.rs:443-459:
```
    // The update, by contrast, IS a change to the detector's configuration, so it
    // moves the timestamp a read reports.
...
    assert!(
        after > before,
        "an update must move the detector's updatedAt: {before:?} -> {after:?}"
    );
```
Nothing pins the current organization behaviour: `grep -n 'updatedAt|GetDetector' tests/all/organization_configuration.rs` → "No matches."

## Steps to Reproduce

1. POST /detector with {"enable": true} to mint a detectorId
2. GET /detector/{detectorId} and note `updatedAt` (call it T0)
3. POST /detector/{detectorId}/admin with {"autoEnableOrganizationMembers": "ALL"} → HTTP 200
4. GET /detector/{detectorId}/admin → the new configuration is reported, so the write landed
5. GET /detector/{detectorId} → `updatedAt` is still T0, although a field of the detector record changed
6. For contrast, POST /detector/{detectorId}/malware-scan-settings with {"ebsSnapshotPreservation": "NO_RETENTION"} and GET /detector/{detectorId} again → `updatedAt` has moved
