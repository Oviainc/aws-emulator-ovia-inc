Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 40f95909aeff149ec63a31404ec4cdfbd5490fd0

# A crossAccountConfigurations entry that names no account passes the own-account rule and is persisted

_Filed: 2026-10-28T23:21:43Z_
_Operation: CreateInvestigationGroup | Class: validation_

> _Note: this issue was identified against the code base at commit `40f95909aeff149ec63a31404ec4cdfbd5490fd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::cross_account_configurations_rejection` is the single home of the rule "a cross-account configuration must name an account OTHER than the investigation account", applied by both writers through `write_configuration_rejection`. Its own doc states the condition under which it passes a list: `None` "when every entry names an account other than this one". The code does not implement that condition — it refuses only an entry that POSITIVELY names the emulator account, and lets through every entry for which the account cannot be read at all:

1. `CrossAccountConfiguration.sourceRoleArn` is NOT `@required` (`smithy_query(query="shape",shape="CrossAccountConfiguration")` reports `sourceRoleArn` with no `required`), so `"crossAccountConfigurations": [{}]` is a request the model admits. `filter_map(|configuration| configuration.source_role_arn.as_deref())` silently SKIPS such an entry, so `.any(..)` is false and the chain returns `None`. The entry — which names no account at all, and so by the helper's own words "could never do anything" — is then stored verbatim by the create (`cross_account_configurations: input.cross_account_configurations.unwrap_or_default()`) and reported back by `GetInvestigationGroup` as a configured cross-account configuration carrying nothing.

2. The account comparison itself is only meaningful for a 6-field ARN. `arn_account_id` splits off the first five fields and returns the FOURTH `next()` as the account, so for a `sourceRoleArn` the model admits but that carries no account field — e.g. `arn:aws:iam::role/CrossAccountInvestigation` (43 characters, matches the modelled `^arn:.*$`, inside `@length 20..2048`) — the resource portion `role/CrossAccountInvestigation` is read AS the account id. The entry therefore never compares equal to the investigation account and always passes, whatever it names.

The result is persisted state the service's own rule says cannot exist: a group whose `crossAccountConfigurations` reports entries that name no account. This is the same class of defect as the already-open "empty encryptionConfiguration" issues, on a different member — those cover `encryptionConfiguration` only, and the open (CreateInvestigationGroup, validation) issue covers `roleArn` assumability only, so neither reaches this path.

## Evidence

The rule's own stated passing condition — `services/service-com-amazonaws-aiops-aiops/src/state.rs:1152-1157`:
```
/// Why a `crossAccountConfigurations` list is unacceptable, or `None` when every
/// entry names an account other than this one.
///
/// A cross-account configuration exists to let the investigating account reach
/// ANOTHER account's resources, so a `sourceRoleArn` whose account id is the
/// investigation account's own is refused: it could never do anything.
```
What the code checks — `src/state.rs:1166-1173`:
```
pub fn cross_account_configurations_rejection(
    configurations: Option<&[CrossAccountConfiguration]>,
) -> Option<String> {
    let own_account = ::engine::EMULATOR_ACCOUNT_ID;
    configurations?
        .iter()
        .filter_map(|configuration| configuration.source_role_arn.as_deref())
        .any(|arn| arn_account_id(arn) == Some(own_account))
```
An entry with no `sourceRoleArn` is dropped by `filter_map`, and the "account" an ARN without an account field yields is its resource portion — `src/state.rs:1241-1249`:
```
pub fn arn_account_id(arn: &str) -> Option<&str> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, _service, _region, account) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    (!account.is_empty()).then_some(account)
```
The model admits both inputs: `smithy_query(query="shape",shape="CrossAccountConfiguration")` -> `{"members": {"sourceRoleArn": {"target": "RoleArn", "length": {"min": 20, "max": 2048}, "pattern": "^arn:.*$"}}}` — no `required`, and the pattern/length admit an ARN with no account field.
The create applies only this chain and then stores the list as given — `src/ops/create_investigation_group.rs:251-256` and `:353`:
```
        if let Some(why) = crate::state::write_configuration_rejection(
            input.encryption_configuration.as_ref(),
            input.cross_account_configurations.as_deref(),
```
```
            cross_account_configurations: input.cross_account_configurations.unwrap_or_default(),
```
and the reader hands it straight back — `src/ops/get_investigation_group.rs:207`:
```
            cross_account_configurations: group.cross_account_configurations_output(),
```
Test coverage: the suite that pins this rule — `tests/all/investigation_group_singleton.rs:895-935` (`/// A cross-account configuration must name an account OTHER than the investigation account`) — builds every row through `.map(|arn| json!({ "sourceRoleArn": arn }))` with a well-formed 6-field ARN, and `tests/all/write_configuration_rejection_chain.rs:267` does the same; a grep of the whole test tree for `sourceRoleArn` shows no row sending `{}` or an ARN without an account field, so both paths are unpinned.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"cross-empty","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","crossAccountConfigurations":[{}]} — 201, note the returned ARN
2. GET /investigationGroups/<url-encoded ARN> signed for us-west-2 — 200 with "crossAccountConfigurations": [{}], an entry that names no account
3. Delete that group and repeat the create in the same Region with "crossAccountConfigurations":[{"sourceRoleArn":"arn:aws:iam::role/CrossAccountInvestigation"}] (no account field; 43 chars, matches ^arn:.*$) — 201, and the entry reads back unchanged
4. Compare both accepted requests with src/state.rs:1152-1157, which says the list passes only when every entry names an account other than the investigation account, and with src/state.rs:1241-1249, which reads 'role/CrossAccountInvestigation' as that entry's account id
