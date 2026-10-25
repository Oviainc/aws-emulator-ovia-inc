Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# Explicit null for visibleServices/visibleRegions does not reset the setting to its default

_Filed: 2026-10-24T01:56:34Z_
_Operation: UpdateAccountCustomizations | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateAccountCustomizations documents two distinct request forms for a list setting: OMITTING the member leaves the stored value unchanged, and sending the member with the JSON value `null` RESETS it to the default (all Regions / all services visible, reported by GetAccountCustomizations as an absent/null member). The emulator implements only the first: a request body carrying `"visibleServices": null` (or `"visibleRegions": null`) is indistinguishable from an omitted member inside the handler, so the previously stored list survives.

Expected (real service): after PATCH /v1/account-customizations with {"visibleServices": ["s3","ec2"]}, a subsequent PATCH with {"visibleServices": null} answers 200 with visibleServices absent/null, and GetAccountCustomizations afterwards also reports it absent/null (feature unconfigured, all services visible).

Actual (emulator): the second PATCH answers 200 echoing the still-stored ["s3","ec2"], and GetAccountCustomizations keeps reporting ["s3","ec2"] forever. There is NO request that can return a configured list setting to its unconfigured/default state — sending an empty array instead stores and reports `[]`, which is a different document (and a different console meaning) from the documented null.

The same applies to visibleRegions. accountColor's documented reset (the `none` value) does work.

## Evidence

Spec: the Smithy model's UpdateAccountCustomizations doc (smithy_query query="operation" operation="UpdateAccountCustomizations"; also the member docs on UpdateAccountCustomizationsInput$visibleServices/$visibleRegions) says "Only the settings that you include in the request body are modified. Omitted settings remain unchanged. To reset a setting to its default behavior, set the value to null for visible Regions and visible services, or none for account color" and "Set to null to reset to the default, which makes all services visible". GetAccountCustomizations' doc confirms the observable default: "visible Regions and visible services return null".

Handler: services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs lines 203-229 — `if let Some(services) = input.visible_services { record.set_visible_services(services) }`, with the comment at lines 205-208 stating "At the typed boundary an omitted member arrives as `None`, so `None` leaves the stored value alone; ... a list [is reset] by sending it empty". The input type (src/ops/types/update_account_customizations_types.rs lines 16-20: `visible_services: Option<Vec<String>>`) collapses explicit JSON null and an omitted member into the same `None`, so the reset branch cannot be reached and the null form is silently dropped. State (src/state.rs lines 14-22, 376-390) models the distinction correctly — `Option<Vec<String>>` with `None` = never configured — so nothing but the request-side discrimination is missing.

Tests: no test in tests/all sends a JSON null for either list member (grep for `null`/`Value::Null` over tests/all finds only doc comments and the response-side helper in wire.rs line 76). tests/all/account_customizations.rs::settings_omitted_from_a_request_remain_unchanged covers only the OMITTED form, and ::update_persists_and_get_reads_back_every_shape_of_setting covers the empty-array form. The open issue 20261022T220228Z (list member ORDER on the update echo) and the fixed uxc issues (catalog contents, nextToken message, body-refusal error code, round-trip of valid identifiers) do not mention the null-reset semantics.

## Steps to Reproduce

1. Boot the emulator with the uxc service mounted.
2. PATCH /v1/account-customizations with body {"visibleServices": ["s3", "ec2"], "visibleRegions": ["us-east-1"]} — 200, both settings stored.
3. PATCH /v1/account-customizations with body {"visibleServices": null} — expect 200 whose visibleServices is absent/null (setting reset to default); the emulator instead echoes ["s3", "ec2"].
4. GET /v1/account-customizations — expect visibleServices absent/null (unconfigured, all services visible); the emulator still reports ["s3", "ec2"].
5. Repeat steps 3-4 with {"visibleRegions": null} for the same divergence on the Regions setting.
6. Note that no request form can restore the unconfigured state: {"visibleServices": []} yields a stored and reported empty array, not the documented null.
