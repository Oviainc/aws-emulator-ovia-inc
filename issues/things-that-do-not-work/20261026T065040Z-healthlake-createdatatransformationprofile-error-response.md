Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# Profile handlers hand-roll an enum refusal that quotes the rejected value, breaking the rule this crate documents and the helper CreateFHIRDatastore uses

_Filed: 2026-10-26T06:50:40Z_
_Operation: CreateDataTransformationProfile | Class: error-response_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for how it refuses a value outside a modelled enum: the clause names the member and the permitted set and quotes NO value, and it owns a helper (`crate::service::enum_violation_message`) so every handler that re-states an enum constraint answers in those same words. `CreateFHIRDatastore` calls that helper. The three data-transformation-profile handlers that re-state the `SourceFormat` enum do not: each one `format!`s its own sentence AND interpolates the rejected value (`got '{}'`), which is exactly what the crate's own doc comment says must not happen. The three copies have therefore already drifted from the service-wide rule and from the helper, so a refusal on these paths would read differently from a refusal of any other enum member of this service. The three sites are `create_data_transformation_profile.rs`, `list_data_transformation_profiles.rs` and `publish_data_transformation_profile.rs`; the fix is the same for all three (call `crate::service::enum_violation_message(<member path>, SOURCE_FORMAT_VALUES)`).

## Evidence

The rule, in this crate's own words — services/service-com-amazonaws-healthlake-healthlake/src/service.rs:147-153: "A value outside an enumerated type's value set is the one class whose clause states the member and the permitted set ALONE — `Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]` — with no echo of what the caller sent: an unknown enum name never becomes a value of the modelled type, so there is no value for the refusal to quote." and src/service.rs:176-177: "// Deliberately absent: this service's enum clause quotes no value."

The sibling that honours it — src/ops/create_fhir_datastore.rs:192-201: "// It answers in the SAME words as the validator — member and permitted set, no echo of the rejected value." … `message: Some(crate::service::enum_violation_message(DATASTORE_TYPE_VERSION_PATH, F_H_I_R_VERSION_VALUES))`.

The three that break it, all echoing the value:
- src/ops/create_data_transformation_profile.rs:199-205: `if !SOURCE_FORMAT_VALUES.contains(&input.source_format.as_str()) { return Err(validation(format!("SourceFormat must be one of {}, got '{}'", SOURCE_FORMAT_VALUES.join(", "), input.source_format))); }`
- src/ops/list_data_transformation_profiles.rs:166-172: same `format!("SourceFormat must be one of {}, got '{}'", …)`
- src/ops/publish_data_transformation_profile.rs:196-202: same `format!("SourceFormat must be one of {}, got '{}'", …)`

Test coverage of the rule (asserted for other enum members, never for `SourceFormat`) — tests/all/control_plane_error_semantics.rs:210 `async fn an_enum_refusal_names_the_member_and_the_set_but_not_the_rejected_value()` and :239-242 `assert!(!message.contains(&format!("'{case}'")), "case {case:?}: the enum clause must not quote the rejected value, got {message:?}")` — so the crate both documents and tests the rule these three handlers contradict.

## Steps to Reproduce

1. Read services/service-com-amazonaws-healthlake-healthlake/src/service.rs:146-179 — the service-wide enum clause rule and the `enum_violation_message` helper written for handlers that re-state an enum constraint.
2. Read src/ops/create_fhir_datastore.rs:189-204 — the sibling handler that re-states its enum through that helper, with no echo of the value.
3. Read src/ops/create_data_transformation_profile.rs:195-205, src/ops/list_data_transformation_profiles.rs:162-172 and src/ops/publish_data_transformation_profile.rs:192-202 — each re-states the `SourceFormat` enum with its own sentence and interpolates the rejected value.
4. Compare with tests/all/control_plane_error_semantics.rs:209-243, which asserts for `DatastoreTypeVersion` and `DatastoreFilter.DatastoreStatus` that an enum refusal of this service never quotes the rejected value.
