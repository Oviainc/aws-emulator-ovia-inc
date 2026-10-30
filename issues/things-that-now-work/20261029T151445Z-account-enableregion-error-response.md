Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d

# The invalidRegionOptTarget ValidationException leaves fieldList unset, although it is refusing the same RegionName member its sibling refusal names

_Filed: 2026-10-29T15:14:45Z_
_Operation: EnableRegion | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f3666ffc56d576d343f5b2efcf2fb2c8ab1775d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::region_opt_target` is the single decision both `EnableRegion` and `DisableRegion` make about the `RegionName` they were handed, and it returns one of two `ValidationException` payloads. `unknown_region` names the offending member in `fieldList`; `region_not_opt_target` — raised for a Region the catalog turns on by default — leaves `field_list: None`, even though the member at fault is the same `RegionName`, refused before anything else in the request is looked at. A client that reads `fieldList` to learn which input was rejected gets it for one of the two refusals of the same member and not for the other.

The crate states its own criterion for leaving `fieldList` unset, and it does not hold here: `no_primary_email_update` omits `reason`/`fieldList` because "no single input member is at fault". For an enabled-by-default Region, exactly one input member is at fault.

## Evidence

Both halves of the same helper, in this crate:

services/service-com-amazonaws-account-account/src/state.rs:578-584
```
pub fn region_opt_target(region_name: &str) -> Result<(), ValidationException> {
    match region_kind(region_name) {
        None => Err(unknown_region(region_name)),
        Some(RegionKind::EnabledByDefault) => Err(region_not_opt_target(region_name)),
        Some(RegionKind::OptIn) => Ok(()),
    }
}
```

The sibling refusal NAMES the member — state.rs:1636-1649:
```
/// `RegionName` is modelled only `@length(min: 1, max: 50)`, so the boundary
/// admits any string of that length: WHICH strings name a Region is a rule this
/// service enforces itself. `reason` is `fieldValidationFailed` and `fieldList`
/// names the member, so the caller can see which input was rejected.
pub fn unknown_region(region_name: &str) -> ValidationException {
    ...
        field_list: Some(vec![ValidationExceptionField {
            name: "RegionName".to_string(),
```

This one does not — state.rs:1652-1665:
```
/// The `ValidationException` payload for a Region that cannot be opted into or
/// out of: it is enabled by default, so it is not a Region-opt target.
...
pub fn region_not_opt_target(region_name: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "Region {region_name} is enabled by default and cannot be enabled or disabled"
        ),
        reason: Some(INVALID_REGION_OPT_TARGET.to_string()),
        field_list: None,
    }
}
```

The crate's stated reason for an unset `fieldList`, which does not apply here — state.rs:1359-1361:
```
/// Neither modelled `reason` covers it (the enum admits only an
/// invalid Region opt target and a failed field validation, and no single input
/// member is at fault), so `reason` and `fieldList` stay unset.
```

The model's own description of the member: `ValidationException$fieldList` — "The field where the invalid entry was detected." (smithy_query(query="shape", shape="ValidationException")).

Both handlers decide through the helper before touching the store: enable_region.rs:203-205 and disable_region.rs:204-206 (`if let Err(invalid) = crate::state::region_opt_target(&region_name)`). Existing coverage: tests/all/region_opt_target_rule.rs pins the two refusals' status/code.

## Steps to Reproduce

1. Start the emulator with the account service mounted.
2. POST /enableRegion with {"RegionName": "zz-nowhere-1"} — observe HTTP 400 ValidationException whose body carries fieldList: [{"name": "RegionName", ...}].
3. POST /enableRegion with {"RegionName": "us-east-1"} (a Region the catalog marks ENABLED_BY_DEFAULT) — observe HTTP 400 ValidationException with reason invalidRegionOptTarget and NO fieldList, although the member refused is again RegionName.
4. Repeat both against /disableRegion: the same asymmetry, since both handlers decide through state::region_opt_target.
