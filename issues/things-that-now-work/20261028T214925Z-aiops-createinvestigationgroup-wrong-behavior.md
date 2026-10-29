Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6249b35117da18b492ce466a823561b75d5aa9f1

# An empty encryptionConfiguration on create persists a group whose encryption state is never reported

_Filed: 2026-10-28T21:49:25Z_
_Operation: CreateInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6249b35117da18b492ce466a823561b75d5aa9f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateInvestigationGroup` normalises the encryption configuration only for the ABSENT member: `input.encryption_configuration.unwrap_or_else(crate::state::default_encryption_configuration)`. `EncryptionConfiguration.type` is not `@required` (`smithy_query(query="shape",shape="EncryptionConfiguration")` reports `type` and `kmsKeyId`, neither required), so `{"encryptionConfiguration": {}}` is a request the model admits. The create path's own configuration-rejection chain has nothing to say about it — `encryption_configuration_rejection` returns `None` at the early `?` as soon as `type` is absent — so the empty structure is stored verbatim as the group's configuration.

The result is a persisted investigation group whose encryption configuration carries neither `type` nor `kmsKeyId`: `GetInvestigationGroup` afterwards reports `"encryptionConfiguration": {}` instead of `{"type":"AWS_OWNED_KEY"}`, so no read ever reports how that group's data is encrypted, and the state cannot be repaired except by a later write that names a type. That is exactly the state the comment directly above the create's own write says cannot exist ("it is stored, so every later read reports how this group's data is encrypted"), the state `default_encryption_configuration`'s doc says it exists to prevent ("rather than the member going missing"), and the state the test pinning this member asserts is impossible ("A group's encryption configuration is always reported").

This is the CREATE writer. The already-open (UpdateInvestigationGroup, wrong-behavior) issue covers the update writer and cites the create path as the one that "guards against exactly the state the update path is able to produce" — which holds only for an OMITTED member; for an empty structure the create writer produces the same unreported state, so normalising only on update would leave this path broken.

## Evidence

The create writer normalises only the absent case — services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:326-333:
```
            // Omitting `encryptionConfiguration` selects the Amazon Web Services owned
            // key, which is a configuration like any other: it is stored, so every
            // later read reports how this group's data is encrypted.
            encryption_configuration: Some(
                input
                    .encryption_configuration
                    .unwrap_or_else(crate::state::default_encryption_configuration),
            ),
```
The rejection chain the create applies deliberately passes a configuration without `type` — services/service-com-amazonaws-aiops-aiops/src/state.rs:984-990:
```
/// `type` is NOT `@required` on the structure, so a configuration that omits it is a
/// request the model admits and this rule has nothing to say about — hence the early
/// `?` on the member rather than a refusal.
pub fn encryption_configuration_rejection(
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let supplied = configuration?.r#type.as_deref()?;
```
The rule the normalisation helper states — services/service-com-amazonaws-aiops-aiops/src/state.rs:1142-1144:
```
/// carries no `kmsKeyId` because no customer managed key is involved — so the
/// default-key state is a stored configuration like any other and every later read
/// reports it, rather than the member going missing.
```
The reader echoes whatever was stored, with no normalisation of its own — services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group.rs:202:
```
            encryption_configuration: group.encryption_configuration.clone(),
```
Test coverage: services/service-com-amazonaws-aiops-aiops/tests/all/investigation_group_singleton.rs:548 states the rule this path breaks — `/// A group's encryption configuration is always reported.` — and its rows (lines 569-588) send the member absent, `{"type":"AWS_OWNED_KEY"}` and the customer-managed pair; a grep of the whole test tree for `encryptionConfiguration` shows no row anywhere sending `{}`, so this path is unpinned. tests/all/write_configuration_rejection_chain.rs:288-350 only exercises `type` values inside and outside the modelled set.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"empty-enc","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","encryptionConfiguration":{}} — 201, note the returned ARN
2. GET /investigationGroups/<url-encoded ARN> signed for us-west-2 — 200 with "encryptionConfiguration": {}
3. Repeat step 1 in another Region WITHOUT the encryptionConfiguration member and GET it back — 200 with "encryptionConfiguration": {"type":"AWS_OWNED_KEY"}
4. Compare the two reads against src/ops/create_investigation_group.rs:326-328 ("every later read reports how this group's data is encrypted") and tests/all/investigation_group_singleton.rs:548 ("A group's encryption configuration is always reported") — the first group's encryption state is never reported
