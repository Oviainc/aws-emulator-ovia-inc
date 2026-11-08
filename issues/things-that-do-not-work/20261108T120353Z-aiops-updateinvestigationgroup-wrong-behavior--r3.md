Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 386e377a24364e2ae13f63408a712b163326bf5e

# UpdateInvestigationGroup that omits encryptionConfiguration never reaches the single-homed stored_encryption_configuration rule, so the documented omission default is not applied

_Filed: 2026-11-08T12:03:53Z_
_Operation: UpdateInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `386e377a24364e2ae13f63408a712b163326bf5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's own doc for `UpdateInvestigationGroupRequest$encryptionConfiguration` states what the service does when the request OMITS the member: it uses an Amazon Web Services key. The handler does the opposite — on an omitted member it leaves the stored configuration completely untouched, so a group previously configured with a customer managed KMS key keeps reporting `CUSTOMER_MANAGED_KMS_KEY` + `kmsKeyId` after an update that named no encryption configuration.

The crate contradicts itself here too. `crate::state::stored_encryption_configuration` is documented as the ONE home of "what a write stores for the configuration it was given", explicitly covering the case of the member being omitted, and claims that BOTH writers (the create and the update) store what it returns so "the two cannot disagree about what a configuration means". The create path honours that: it calls the helper unconditionally, passing `input.encryption_configuration` even when it is `None`, which is how the omitted-member branch of the helper is reached. The update path guards the call with `if input.encryption_configuration.is_some()`, so the helper's omitted-member branch is unreachable from the update and the two writers DO disagree about what an omitted member means.

No test pins the update's retain-on-omit behaviour: the suite that drives every configuration form through both writers explicitly SKIPS the omitted-member row for the update.

## Evidence

Smithy (quoted verbatim, `smithy_query(query="shape",shape="UpdateInvestigationGroupRequest")`, member `encryptionConfiguration`): "Use this structure if you want to use a customer managed KMS key to encrypt your investigation data. If you omit this parameter, CloudWatch investigations will use an Amazon Web Services key to encrypt the data." — the same sentence is carried in the handler file itself at services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:65-68: "//! * `encryptionConfiguration` (EncryptionConfiguration) — Use this structure if you want to / //!   use a customer managed KMS key to encrypt your investigation data. If you omit this / //!   parameter, CloudWatch investigations will use an Amazon Web Services key to encrypt the / //!   data."

Handler, services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs:275-279:
```
        if input.encryption_configuration.is_some() {
            group.encryption_configuration = Some(crate::state::stored_encryption_configuration(
                input.encryption_configuration,
            ));
        }
```
(and the comment above it, line 262-264: "Each member the request carries replaces the stored value; each one it omits / leaves the stored value untouched.")

The rule the update bypasses, services/service-com-amazonaws-aiops-aiops/src/state.rs:1226-1228: "* a request that names NO key at all — the member omitted, or a structure carrying / ///   neither member — asks for no customer managed key and gets the documented Amazon / ///   Web Services owned key ([`default_encryption_configuration`])." and state.rs:1233-1235: "Single-homed here because every writer of the member — the create and the update — / /// stores what this returns, so the two cannot disagree about what a configuration / /// means."

The sibling writer that DOES apply it, services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:356-358:
```
            encryption_configuration: Some(crate::state::stored_encryption_configuration(
                input.encryption_configuration,
            )),
```

Test coverage: services/service-com-amazonaws-aiops-aiops/tests/all/stored_encryption_configuration.rs:277-279 — "// Skip the omitted-member row here: an update that omits the member leaves the / // stored configuration alone, which `rows` does not describe." — so the update's omitted-member case is asserted nowhere; every other row is driven through both writers.

## Steps to Reproduce

1. POST /investigationGroups signed for us-west-2 with {"name":"g","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole","encryptionConfiguration":{"type":"CUSTOMER_MANAGED_KMS_KEY","kmsKeyId":"arn:aws:kms:us-west-2:000000000000:key/abcd"}} -> 201, note the ARN
2. PATCH /investigationGroups/<url-encoded ARN> with a body that names no encryptionConfiguration at all, e.g. {"isCloudTrailEventHistoryEnabled":false} -> 200
3. GET /investigationGroups/<url-encoded ARN> -> encryptionConfiguration still reports {"type":"CUSTOMER_MANAGED_KMS_KEY","kmsKeyId":"..."}, although the member's own documentation for this operation says that omitting the parameter means the Amazon Web Services key is used
4. Compare with CreateInvestigationGroup, which routes the same omitted member through crate::state::stored_encryption_configuration and stores AWS_OWNED_KEY
