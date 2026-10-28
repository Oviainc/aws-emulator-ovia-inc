Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ef3dfae520d72d664802e5111b62d3d016960f9c

# An empty encryptionConfiguration on update erases the group's stored key type, so a later read no longer reports how its data is encrypted

_Filed: 2026-10-28T16:18:06Z_
_Operation: UpdateInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ef3dfae520d72d664802e5111b62d3d016960f9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateInvestigationGroup` replaces the stored `encryptionConfiguration` with whatever structure the request carried, without the normalisation the create path applies. `EncryptionConfiguration.type` is not `@required`, so `{"encryptionConfiguration":{}}` is a request the model admits, it passes the configuration-rejection chain untouched (the chain returns `None` as soon as `type` is absent), and the handler stores the empty structure verbatim.

The result is a stored group whose encryption configuration carries neither `type` nor `kmsKeyId`: `GetInvestigationGroup` afterwards reports `"encryptionConfiguration": {}` where it previously reported `{"type":"AWS_OWNED_KEY"}` (or the customer-managed pair). The stored key type is lost and cannot be recovered except by another update that names a type.

This contradicts the crate's own single-homed rule for this member: `CreateInvestigationGroup` deliberately substitutes the AWS-owned-key default for an absent configuration precisely so that "every later read reports how this group's data is encrypted" and so that the member never "goes missing". The create path guards against exactly the state the update path is able to produce, and the update handler is the only writer of this member that skips the normalisation helper (`crate::state::default_encryption_configuration`) the crate already has.

## Evidence

Update stores the structure as given, with no normalisation — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:220-222:
```
        if let Some(encryption_configuration) = input.encryption_configuration {
            group.encryption_configuration = Some(encryption_configuration);
        }
```
The rejection chain explicitly has nothing to say about a configuration without `type` — src/state.rs:784-790:
```
/// `type` is NOT `@required` on the structure, so a configuration that omits it is a
/// request the model admits and this rule has nothing to say about — hence the early
/// `?` on the member rather than a refusal.
pub fn encryption_configuration_rejection(
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let supplied = configuration?.r#type.as_deref()?;
```
The sibling write path normalises, and states the rule the update breaks — src/ops/create_investigation_group.rs:309-316:
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
and src/state.rs:941-944 for the helper the update path never reaches:
```
/// carries no `kmsKeyId` because no customer managed key is involved — so the
/// default-key state is a stored configuration like any other and every later read
/// reports it, rather than the member going missing.
```
Test coverage: tests/all/investigation_group_singleton.rs:557 `an_omitted_encryption_configuration_reads_back_as_the_aws_owned_key` pins that an omitted configuration reads back as `{"type":"AWS_OWNED_KEY"}` (rows at lines 569-588) and that an update naming the member reports the replacement back (lines 629-647) — but every row sends a structure that carries a `type`; no test sends `{}`, so this path is unpinned. tests/all/write_configuration_rejection_chain.rs:288-350 only exercises `type` values inside and outside the modelled set.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"enc-erase","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} and note the returned ARN
2. GET /investigationGroups/<url-encoded ARN> -> 200 with "encryptionConfiguration": {"type":"AWS_OWNED_KEY"}
3. PATCH /investigationGroups/<url-encoded ARN> with {"encryptionConfiguration":{}} -> 200
4. GET /investigationGroups/<url-encoded ARN> again -> 200 with "encryptionConfiguration": {} — the stored key type has been erased and the read no longer reports how the group's data is encrypted
