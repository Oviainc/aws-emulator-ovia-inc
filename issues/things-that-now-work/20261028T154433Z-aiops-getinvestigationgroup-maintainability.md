Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# ARN field splitting is implemented twice in state.rs instead of single-homed

_Filed: 2026-10-28T15:44:32Z_
_Operation: GetInvestigationGroup | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate reads fields out of an ARN in two hand-rolled places in `src/state.rs`, each encoding the same grammar rule — an ARN is `arn:partition:service:region:account:resource` and the RESOURCE portion may itself contain `:`, so only the leading fields may be split off — with different arities and different handling of the `arn:` prefix.

`group_id_from_arn` (reached from every identifier/resourceArn resolution: `resolve_group_arn` -> `group`/`group_mut`/`remove_group`, i.e. GetInvestigationGroup, UpdateInvestigationGroup, DeleteInvestigationGroup, the policy operations and the three tagging operations) checks `starts_with("arn:")` and then does `splitn(6, ':')`, keeping field 0 as the literal `arn`. `arn_account_id` (reached from `cross_account_configurations_rejection` -> `write_configuration_rejection`, i.e. CreateInvestigationGroup and UpdateInvestigationGroup) instead does `strip_prefix("arn:")?` and then `splitn(5, ':')`, so the same six-field grammar is expressed as five fields over a stripped string.

The two copies can drift: any change to how this service reads an ARN — tolerating an empty service field, treating the prefix differently, or handling a resource portion that contains colons — has to be made twice, and a change made in one place leaves the other on the old grammar (e.g. a resolver that accepts an ARN form the cross-account rule cannot read the account id out of, or the reverse). That contradicts this module's own stated doctrine, which insists each ARN-shape rule be decided exactly once.

The fix is a single private field splitter in `crate::state` that both functions call. Every message, status and response body stays byte-identical and no existing test changes.

## Evidence

Copy 1 — `services/service-com-amazonaws-aiops-aiops/src/state.rs:1032-1045`:
```
/// The account id field of `arn`, or `None` when `arn` is not an ARN carrying one.
///
/// An ARN is `arn:partition:service:region:account:resource`, and the resource
/// portion may itself hold `:`, so only the first five fields are split off.
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, _service, _region, account) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    (!account.is_empty()).then_some(account)
}
```
Copy 2 — the same grammar, re-split with a different arity and prefix handling, `src/state.rs:1094-1113`:
```
/// The id segment of an investigation-group ARN, or `None` when `identifier` is not
/// an ARN of this service's investigation-group resource.
pub fn group_id_from_arn(identifier: &str) -> Option<&str> {
    if !identifier.starts_with("arn:") {
        return None;
    }
    let mut fields = identifier.splitn(6, ':');
    let (_arn, _partition, service, _region, _account, resource) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(GROUP_ARN_RESOURCE_PREFIX)?;
    is_group_id(id).then_some(id)
}
```
The crate's own single-homing doctrine for exactly this kind of ARN-shape knowledge — `src/state.rs:24-25`:
```
//! What an id looks like is likewise decided once, in [`is_group_id`], from the one
//! modelled pattern segment that states it.
```
and `src/state.rs:1086-1089`:
```
/// Everything that has
/// to judge whether some text could name a stored group — the ARN resolver below and
/// the opaque page token a regional reader hands back — judges it here, so the rule is
/// read off that pattern in exactly one place.
```
Call sites proving both copies are live: `src/state.rs:228-233` (`resolve_group_arn` calls `group_id_from_arn`, reached by `src/ops/get_investigation_group.rs:188` `store.group(&input.identifier, &region)`) and `src/state.rs:966-973` (`cross_account_configurations_rejection` calls `arn_account_id`, reached by `src/ops/create_investigation_group.rs:234-239` / `src/ops/update_investigation_group.rs:188-193` through `write_configuration_rejection`).

Test coverage both copies must keep unchanged: `tests/all/regional_identifier_resolution.rs` and `tests/all/investigation_group_identifier_forms.rs` (ARN-form resolution and the 404 for every non-ARN form), `tests/all/group_reader_and_id_shape.rs` (the id-shape rule and the ARN resolver), and `tests/all/write_configuration_rejection_chain.rs` (the cross-account same-account refusal that reads the account id).

## Steps to Reproduce

1. Open src/state.rs:1032-1045 and src/state.rs:1094-1113 side by side: both split an ARN's colon-separated fields, one as `strip_prefix("arn:") + splitn(5, ':')` and the other as `starts_with("arn:") + splitn(6, ':')`
2. Change the ARN grammar in one copy only (for example accept an ARN whose service field is empty, or stop requiring the literal `arn:` prefix) and observe the other copy keeps the old grammar, so identifier resolution and the cross-account account-id rule disagree about what an ARN is
3. Run the crate's tests: tests/all/regional_identifier_resolution.rs, tests/all/investigation_group_identifier_forms.rs, tests/all/group_reader_and_id_shape.rs and tests/all/write_configuration_rejection_chain.rs exercise both copies, so a single shared splitter must keep every current message and status
