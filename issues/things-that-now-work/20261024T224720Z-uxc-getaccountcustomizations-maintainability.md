Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6

# State the "unconfigured accountColor reports none" rule once instead of in both handlers

_Filed: 2026-10-24T22:47:20Z_
_Operation: GetAccountCustomizations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule that an account which never configured a colour is REPORTED as the wire value `none` is implemented twice, once in each handler that renders the customization record: `GetAccountCustomizations` and `UpdateAccountCustomizations` each take the stored `Option<String>` and expand `None` into `crate::state::ACCOUNT_COLOR_NONE` inline. The two copies are the only places this rule exists, so a change to how an unconfigured colour is reported has to be made in both and can silently drift to two different answers for the same stored record (Get saying one thing, the Update echo another) — exactly the drift this crate already avoids for the sibling rule on the list settings, which is stated ONCE in `crate::state` (`canonical_members`) and called by both setters "because the service keeps them alike". The fix is to state the colour rule in the same place (e.g. an accessor on `AccountUxCustomization` next to `set_visible_services`/`set_visible_regions`) and have both handlers call it; every response byte and every existing test stays identical.

## Evidence

Copy 1 — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/get_account_customizations.rs:172-180:
```
        Ok(GetAccountCustomizationsOutput {
            // "A value of none indicates that you have not set a color": an
            // account that never configured one reports `none`, not an absent
            // member.
            account_color: Some(
                stored
                    .account_color
                    .unwrap_or_else(|| crate::state::ACCOUNT_COLOR_NONE.to_string()),
            ),
```
Copy 2 — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/update_account_customizations.rs:234-239:
```
        Ok(UpdateAccountCustomizationsOutput {
            account_color: Some(
                stored
                    .account_color
                    .unwrap_or_else(|| crate::state::ACCOUNT_COLOR_NONE.to_string()),
            ),
```
`grep` over `src/` shows these are the only two uses of `ACCOUNT_COLOR_NONE` outside its definition (src/state.rs:90: `pub const ACCOUNT_COLOR_NONE: &str = "none";`).

The crate's own precedent for stating such a rule once — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/state.rs:32-37:
```
    /// Store the visible-Region codes a request supplied, under the same rule as
    /// [`AccountUxCustomization::set_visible_services`]: one rule for both list
    /// settings, because the service keeps them alike.
    pub fn set_visible_regions(&mut self, supplied: Vec<String>) {
        self.visible_regions = Some(canonical_members(supplied));
    }
```

Test coverage that pins both copies' current output (so a single-statement refactor is verifiable): tests/all/account_customizations.rs:26-41 `get_before_any_update_returns_the_documented_defaults` asserts `accountColor` is `"none"` on a fresh account; tests/all/list_setting_order.rs:411-440 `a_partial_update_reports_the_other_settings_unchanged` asserts the Update echo's `accountColor`; tests/all/visible_services_round_trip.rs:162-171 asserts the Get body carries `"accountColor": "none"` for an account that only set the lists; tests/all/body_validation_refusals.rs:180-200 asserts each of the ten modelled colours is echoed back by Update.

## Steps to Reproduce

1. grep -n 'ACCOUNT_COLOR_NONE' services/service-com-amazonaws-uxc-awsaccountuxsetting/src — two call sites, one per handler, each expanding the stored `None` inline.
2. Read src/ops/get_account_customizations.rs:172-180 and src/ops/update_account_customizations.rs:234-239: the same `Option<String>` -> reported-colour computation, written out twice.
3. Contrast with src/state.rs:24-37, where the equivalent list rule is stated once (`canonical_members`) and shared by both setters.
4. Change the reported default in one copy only (e.g. drop the `unwrap_or_else`) and observe that the two operations now report different colours for the same stored record, with no single place that owns the rule.
