Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: ce65be7ed23a4ac4ad17d64817768d50cbccb885

# An encryptionConfiguration carrying only kmsKeyId is discarded and the group is stored as AWS_OWNED_KEY

_Filed: 2026-11-06T12:06:27Z_
_Operation: CreateInvestigationGroup | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `ce65be7ed23a4ac4ad17d64817768d50cbccb885`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::stored_encryption_configuration` throws away the WHOLE supplied `encryptionConfiguration` structure whenever its `type` member is absent, and substitutes `{type: AWS_OWNED_KEY, kmsKeyId: None}`. Because `type` is not `@required` on the structure, a request may legitimately carry `encryptionConfiguration: {"kmsKeyId": "arn:aws:kms:<region>:<account>:key/<id>"}` — a request that names a customer managed KMS key and nothing else. That request is accepted with 201, the caller-supplied `kmsKeyId` is never stored anywhere, and `GetInvestigationGroup` afterwards reports `encryptionConfiguration: {"type": "AWS_OWNED_KEY"}` — i.e. the emulator answers that the group's investigation data is encrypted with an Amazon Web Services owned key for a caller who asked for their own key, with no refusal and no trace of the key they named.

The model's conditional for that default is specifically the ABSENCE of the parameter (`encryptionConfiguration`: "If you omit this parameter, CloudWatch investigations will use an Amazon Web Services key to encrypt the data"), and the member's first sentence is "Use this structure if you want to use a customer managed KMS key to encrypt your investigation data". A structure carrying `kmsKeyId` does not omit the parameter and does ask for a customer managed key, so the handler's reasoning — "A configuration that names no type therefore asks for no customer managed key" (state.rs:1213-1215) — does not hold for this input: the one member that names the customer managed key is the one being dropped.

The same helper is the single home for both writers, so `UpdateInvestigationGroup` drops the member the same way (update_investigation_group.rs:274-278). Either the pair should be refused with the modelled `ValidationException` or the supplied `kmsKeyId` must survive the write; silently dropping it and asserting the opposite encryption state is what is wrong. Note the neighbouring behaviour — the member omitted entirely, or a structure carrying no member at all — is correct and is NOT what this issue asks to change.

## Evidence

Smithy, `CreateInvestigationGroupInput$encryptionConfiguration` (`smithy_query(query="doc",shape="CreateInvestigationGroupInput",member="encryptionConfiguration")`):

> "Use this structure if you want to use a customer managed KMS key to encrypt your investigation data. If you omit this parameter, CloudWatch investigations will use an Amazon Web Services key to encrypt the data."

Smithy, `EncryptionConfiguration$kmsKeyId` (`smithy_query(query="doc",shape="EncryptionConfiguration",member="kmsKeyId")`):

> "If the investigation group uses a customer managed key for encryption, this field displays the ID of that key."

Smithy, `EncryptionConfiguration` (`smithy_query(query="shape",shape="EncryptionConfiguration")`) — neither member is `@required`, so a `kmsKeyId`-only structure passes the boundary walk:
```
"EncryptionConfiguration": {"type": "structure", "members": {"type": {"target": "EncryptionConfigurationType", "type": "enum", ...}, "kmsKeyId": {"target": "KmsKeyId", "length": {"min": 1, "max": 256}, "pattern": "^arn:.*$", ...}}, "doc": "<p>Use this structure to specify a customer managed KMS key to use to encrypt investigation data. </p>"}
```

The code that drops it — services/service-com-amazonaws-aiops-aiops/src/state.rs:1222-1228:
```
pub fn stored_encryption_configuration(
    supplied: Option<EncryptionConfiguration>,
) -> EncryptionConfiguration {
    supplied
        .filter(|configuration| configuration.r#type.is_some())
        .unwrap_or_else(default_encryption_configuration)
}
```
and what it substitutes — state.rs:1199-1204:
```
pub fn default_encryption_configuration() -> EncryptionConfiguration {
    EncryptionConfiguration {
        r#type: Some(AWS_OWNED_KEY_TYPE.to_string()),
        kms_key_id: None,
    }
}
```
Its own stated reason, which the `kmsKeyId`-bearing input contradicts — state.rs:1211-1217:
```
/// name the member while naming no key type at all — and `encryptionConfiguration` is
/// documented as the way to ask for a customer managed KMS key, with the Amazon Web
/// Services owned key used when the request does not ask for one. A configuration that
/// names no type therefore asks for no customer managed key and selects the same
/// documented default as omitting the member does
```

The create path that stores the substituted value — services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:354-356:
```
            encryption_configuration: Some(crate::state::stored_encryption_configuration(
                input.encryption_configuration,
            )),
```
and the update path, through the same helper — services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:274-278:
```
        if input.encryption_configuration.is_some() {
            group.encryption_configuration = Some(crate::state::stored_encryption_configuration(
                input.encryption_configuration,
            ));
        }
```
The read that reports it back as fact — ops/get_investigation_group.rs:202:
```
            encryption_configuration: group.encryption_configuration.clone(),
```

Test coverage: the behaviour is pinned by services/service-com-amazonaws-aiops-aiops/tests/all/stored_encryption_configuration.rs:224-228:
```
        Row {
            case: "a kmsKeyId with no type",
            supplied: Some(json!({ "kmsKeyId": key })),
            expected: default.clone(),
        },
```
with `default = json!({ "type": AWS_OWNED_KEY_TYPE })` (line 212) — i.e. the test asserts that the caller's key id is gone. Its stated reason (same file, lines 11-16) is "the Amazon Web Services owned key is used when a request does not ask for one — so a write whose configuration names no key type ... stores that documented default", which is exactly the reading the two model sentences above contradict for an input that DOES name a key. tests/all/create_encryption_configuration_forms.rs:14-20 spans the same forms with the same expectation. No test asserts that a supplied `kmsKeyId` is either preserved or refused.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with body {"name":"enc-key-only","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","encryptionConfiguration":{"kmsKeyId":"arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555"}} — the emulator answers 201 and returns the new ARN
2. GET /investigationGroups/<url-encoded ARN> — the response reports encryptionConfiguration as {"type":"AWS_OWNED_KEY"}; the kmsKeyId the create named is absent from the stored record entirely, so the group reports Amazon-Web-Services-owned encryption for a request that named a customer managed key
3. PATCH /investigationGroups/<url-encoded ARN> with {"encryptionConfiguration":{"kmsKeyId":"arn:aws:kms:us-west-2:000000000000:key/11111111-2222-3333-4444-555555555555"}} on a group previously created with {"type":"CUSTOMER_MANAGED_KMS_KEY","kmsKeyId":<key>} — 200, and the following GET reports {"type":"AWS_OWNED_KEY"}: the same helper replaced the stored customer-managed configuration with the AWS-owned default even though the request named a key
4. Compare with the model: CreateInvestigationGroupInput$encryptionConfiguration documents the Amazon Web Services key for the case where you "omit this parameter", which step 1 did not do, and EncryptionConfiguration$kmsKeyId documents the member as the id of the customer managed key the group uses
