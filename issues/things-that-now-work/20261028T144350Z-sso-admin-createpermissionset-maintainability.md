Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7c677bf184452d3016cbf464b9a60cef63f91cb5

# Fold the permission-set-surface resolution (account-instance gate → instance lookup → permission-set lookup) into one state.rs helper, as the tag surface already is

_Filed: 2026-10-28T14:43:50Z_
_Operation: CreatePermissionSet | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7c677bf184452d3016cbf464b9a60cef63f91cb5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of the permission-set / account-assignment / ABAC surface opens with the same three-step resolution, hand-copied into each handler:

1. the account-instance withdrawal gate (`store.permission_set_surface_withdrawn(&input.instance_arn)` / `abac_surface_withdrawn`) answered with `ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE` on the `ValidationException` channel — 34 handlers;
2. the instance lookup (`store.instance(&input.instance_arn).is_none()`) answered with `crate::state::instance_not_found_message(..)`;
3. where the request names one, the permission-set lookup answered with `"No permission set found with the ARN {}"` — 20 handlers.

This crate already solved exactly this problem once, for the TAG surface: `state::...::tag_target` / `tag_target_mut` own the whole resolution and hand back a `TagWriteRefusal`, and each of the three tag handlers only maps that refusal onto the variants its own model declares. `state.rs` states that as the pattern for a resolution shared by operations with different error enums. The permission-set surface needs the identical shape (a resolution returning a refusal enum with the three cases above) and does not have it, so 34 copies of one rule can drift — and this crate has already paid for that drift once, in the FIXED issue where the same instance-not-found condition had grown two different `Message` texts across the two halves of this very surface.

The fix changes no response: every site uses the same three messages on the same channels (`ValidationException` for the gate, `ResourceNotFoundException` for both lookups), so folding them behind one state.rs resolution leaves every status, `__type` and `Message` byte-identical and every existing test passing.

## Evidence

The pattern this crate states for a resolution shared across operations with different error enums — `src/state.rs:743-751`:
```
/// Why a tag WRITE could not reach the resource its `ResourceArn` named, as resolved by
/// [`ComAmazonawsSsoadminSwbexternalserviceState::tag_target_mut`].
///
/// Each operation declares its own error enum, so the resolution answers the refusal and
/// the handler maps it onto the variant its model carries — the division
/// [`ComAmazonawsSsoadminSwbexternalserviceState::add_region`] and [`cursor_after`]
/// already keep.
```
and the helper that carries it — `src/state.rs:3073-3076` / `3101-3114`:
```
    /// THE resolution the whole tag surface shares, for an operation that REPORTS the tag
    /// set: the tags of the resource a tag request names, or the message of the
    /// not-found its caller models.
...
    pub fn tag_target_mut(
```
with the handlers reduced to the mapping — `src/ops/tag_resource.rs:187-207`:
```
        // The resource a tag request names — and whether its holding instance is far
        // enough along to take a write at all — is resolved by the ONE rule the whole tag
        // surface shares (`crate::state::...::tag_target_mut`); this handler only maps the
        // refusal it answers onto the errors THIS operation models.
        let stored = store
            .tag_target_mut(input.instance_arn.as_deref(), &input.resource_arn)
            .map_err(|refusal| match refusal {
```
(the same three lines appear in `src/ops/untag_resource.rs:184-206` and `src/ops/list_tags_for_resource.rs:199-214`).

The copies on the permission-set surface. `src/ops/list_permission_sets.rs:186-214`:
```
            if store.permission_set_surface_withdrawn(&input.instance_arn) {
                return Err(ListPermissionSetsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(
                            crate::state::ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE.to_string(),
...
            if store.instance(&input.instance_arn).is_none() {
                return Err(ListPermissionSetsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::instance_not_found_message(
```
`src/ops/delete_permission_set.rs:173-214` — the same two steps plus the third:
```
        if store.permission_set_surface_withdrawn(&input.instance_arn) {
...
        if store.instance(&input.instance_arn).is_none() {
...
        if store
            .permission_set(&input.instance_arn, &input.permission_set_arn)
            .is_none()
        {
            return Err(DeletePermissionSetErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "No permission set found with the ARN {}",
```
`src/ops/create_permission_set.rs:188-222` repeats steps 1 and 2 verbatim; `src/ops/put_inline_policy_to_permission_set.rs:179-224`, `src/ops/provision_permission_set.rs:205-251`, `src/ops/list_account_assignments.rs:201-240` and the rest do the same.

The counts: `grep -n ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE src/ops` reports 34 handlers (every `*PermissionSet*`, `*AccountAssignment*` and `*InstanceAccessControlAttributeConfiguration*` operation); `grep -n "No permission set found with the ARN" src/ops` reports 20.

That the drift is real for this exact rule is on record: the FIXED issue `20261027T222322Z-sso-admin-createpermissionset-error-response` found the step-2 message had grown two different texts across these same handlers, and `20261027T222322Z-sso-admin-tagresource-maintainability` (also fixed) is the same finding for the tag surface — "The taggable-resource resolution rule is written out three times across the tag surface instead of once" — which is what produced `tag_target`/`tag_target_mut`.

Test coverage that must stay green and unchanged: `tests/all/account_instance_restriction.rs` (the step-1 refusal across the surface), `tests/all/unknown_instance_message.rs` and `tests/all/discovered_instance_surface.rs` (step 2), `tests/all/ssoadmin_permission_sets.rs` and `tests/all/permission_set_update.rs` (step 3), `tests/all/validation_message_wording.rs` (the refusal wording).

## Steps to Reproduce

1. `grep -n "permission_set_surface_withdrawn\\|abac_surface_withdrawn" services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops` — 34 handlers each open with the identical gate and the identical `ACCOUNT_INSTANCE_UNSUPPORTED_MESSAGE` refusal.
2. `grep -n "No permission set found with the ARN" services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops` — 20 handlers each hand-roll the third step with the same text.
3. Read `src/state.rs:3073-3114` and `src/ops/tag_resource.rs:187-207`: the tag surface's equivalent resolution lives in ONE place and each handler only maps the refusal onto its own modelled variants.
4. Compare with `src/ops/list_permission_sets.rs:186-214` and `src/ops/delete_permission_set.rs:173-214`: the same decision sequence is written out per handler, so a change to any of the three refusals has to be made 20-34 times.
