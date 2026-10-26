Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b314e0f3433a6118e8049d188afadfe9e3c07a46

# Automatic-snapshot add-on accepts a snapshotTimeOfDay that is not an hourly increment

_Filed: 2026-10-26T04:11:02Z_
_Operation: EnableAddOn | Class: validation_

> _Note: this issue was identified against the code base at commit `b314e0f3433a6118e8049d188afadfe9e3c07a46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AutoSnapshotAddOnRequest$snapshotTimeOfDay` documents two constraints: the value "Must be in `HH:00` format, and in an hourly increment", specified in UTC. The modelled `TimeOfDay` pattern `^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$` only enforces that the value is a valid wall-clock time — it admits `14:30` — so the centralized constraint walk cannot refuse a non-hourly value, and the handler path does not either: `add_ons_from_requests` copies whatever the request carried straight onto the stored add-on, and `GetInstance`/`GetDisk` then report `snapshotTimeOfDay: "14:30"` as the daily automatic snapshot time.

The crate already treats this class of prose constraint as its own responsibility: `is_auto_snapshot_date` exists precisely because the `restoreDate` members' "Must be specified in YYYY-MM-DD format" is prose the modelled walk does not cover. The hourly-increment half of `snapshotTimeOfDay`'s documented constraints is the same kind of rule and is left unenforced, and the `EnableAddOn` handler's own enumeration of "what the model cannot express" omits it.

## Evidence

Model (smithy_query shape com.amazonaws.lightsail#AutoSnapshotAddOnRequest, member snapshotTimeOfDay): "The daily time when an automatic snapshot will be created." / "Constraints:" / "Must be in <code>HH:00</code> format, and in an hourly increment." — with `"pattern": "^(0[0-9]|1[0-9]|2[0-3]):[0-5][0-9]$"`, which admits `14:30`.

Handler/state path that stores it unchecked — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:12376-12385:
```
            "AutoSnapshot" => Ok(AddOn {
                name: Some("AutoSnapshot".to_string()),
                status: Some("Enabled".to_string()),
                snapshot_time_of_day: Some(
                    request
                        .auto_snapshot_add_on_request
                        .as_ref()
                        .and_then(|auto| auto.snapshot_time_of_day.clone())
                        .unwrap_or_else(|| default_snapshot_time_of_day(resource_name)),
                ),
```

The handler's enumeration that omits the rule — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/enable_add_on.rs:170-176:
```
        // `resourceName` and `addOnRequest` are modelled `@required` members,
        // the name carries the modelled `ResourceName` pattern, `addOnType` is a
        // modelled enum and `snapshotTimeOfDay` carries the modelled `TimeOfDay`
        // pattern, so the centralized walk refused anything outside them before
        // this body ran. What is left is what the model cannot express: the name
        // must be one of this account's add-on-carrying resources, and the
        // add-on must be one that kind of resource can carry.
```

The sibling rule the crate DOES enforce for a prose-only format — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:12445-12454:
```
/// Whether `value` is a date in the `YYYY-MM-DD` form an automatic snapshot is
/// addressed by.
///
/// The form is the modelled `AutoSnapshotDate` pattern
/// `^[0-9]{4}-[0-9]{2}-[0-9]{2}$`; the members that carry a restore date
/// (`CreateDiskFromSnapshot$restoreDate` and its siblings) target a plain
/// `string`, whose documentation says "Must be specified in YYYY-MM-DD format",
/// so the constraint is enforced here rather than by the modelled-constraint
/// walk.
pub fn is_auto_snapshot_date(value: &str) -> bool {
```

Test coverage: tests/all/resource_add_ons.rs and tests/all/add_on_disable.rs exercise enabling/disabling the add-on; neither sends a non-hourly `snapshotTimeOfDay`, so no test pins the current acceptance.

## Steps to Reproduce

1. CreateInstances with instanceNames=["add-on-instance"], availabilityZone="us-east-1a", a valid blueprintId/bundleId.
2. Call EnableAddOn with resourceName="add-on-instance" and addOnRequest={"addOnType":"AutoSnapshot","autoSnapshotAddOnRequest":{"snapshotTimeOfDay":"14:30"}} — the call succeeds (HTTP 200) although the documented constraint requires the HH:00 hourly increment.
3. Call GetInstance with instanceName="add-on-instance" — the add-on is reported with snapshotTimeOfDay "14:30".
4. The same unchecked value is accepted through CreateInstances/CreateDisk's `addOns` list, which go through the same `add_ons_from_requests` helper.
