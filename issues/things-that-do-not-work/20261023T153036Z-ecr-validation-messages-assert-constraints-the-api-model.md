Issue-Source: trace-sweep-divergence
Issue-Trace: api-account-settings-and-authorization-token
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-account-settings-and-authorization-token api-lifecycle-policy-preview-and-apply ecr-put-signing-configuration-invalid-profile-arn ecr-put-signing-configuration-invalid-repository-filter ecr-signing-configuration-crud-lifecycle ecr-signing-configuration-invalid-input-then-recover ecr-signing-configuration-invalid-profile-arn
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|GetAccountSetting|4xx->4xx|$.message
Issue-Key: ecr|PutLifecyclePolicy|4xx->4xx|$.message
Issue-Key: ecr|PutSigningConfiguration|4xx->4xx|$.message
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# ECR validation messages assert constraints the API model does not define

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSetting

## Description
This issue bundles 3 DISTINCT root causes. All three are parameter-validation rejections where the status (400) and the error code already match, so the only divergence is the message — and here the message rule IS known: AWS builds these from the request to the fixed template "Invalid parameter at '<Parameter>' failed to satisfy constraint: '<constraint>'", naming the API parameter and the constraint it failed. The emulator either breaks that template or asserts a constraint that is not in the service model. All three must be closed before submitting; each carries its own fixture and step.

Cause 1 of 3: GetAccountSetting rejects an unknown setting name by quoting an enum value set the model does not declare.
AWS: Smithy ecr#GetAccountSetting takes name targeting ecr#AccountSettingName, which is a STRING with length 1..64 and NO enum constraint, and models InvalidParameterException (client, HTTP 400). An unrecognised name is therefore not an enum-membership failure: the recorded rejection is "Invalid parameter at 'name' failed to satisfy constraint: 'Invalid account setting name provided'" — a stable phrase that names no value set.
Emulator: same status and code, but the constraint reads "Member must satisfy enum value set: [BASIC_SCAN_TYPE_VERSION, BLOB_MOUNTING, REGISTRY_POLICY_SCOPE]", i.e. it validates against a hard-coded list that is not a constraint of the modeled shape and publishes that list in the response.
Why: the message is the only machine-readable detail this error carries, since the code is shared by every parameter problem. Clients and operators are told the member failed a constraint the API does not have, and the emulator's internal allow-list is leaked to callers — it will also drift from AWS's real set as settings are added, producing a rejection AWS would not issue (and vice versa).
Fix: in the GetAccountSetting handler's validation, emit the stable constraint phrase for an unrecognised setting name instead of rendering a synthetic enum-membership failure. (Open issue 20261022T002237Z-ecr-getaccountsetting-stub.md tracked this operation as an unimplemented stub; it now answers, and that stub issue does not cover this message rule.)
Traces: api-account-settings-and-authorization-token#task#7.

Cause 2 of 3: PutLifecyclePolicy abandons AWS's parameter-and-constraint template for a lifecycle-document rejection.
AWS: Smithy ecr#PutLifecyclePolicy models InvalidParameterException (client, HTTP 400). A lifecycle document that does not validate is rejected with the template filled from the request: "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: ...'" — the API-documented capitalised parameter label plus a schema-validation sentence.
Emulator: same status and code, but the message does not use the template at all: "lifecyclePolicyText is not a valid lifecycle policy: rule 1 carries 'selection.countType' \"notARealCountType\", which is neither \"imageCountMoreThan\" nor \"sinceImagePushed\"" — the wire member name in lowerCamel and bespoke prose.
Why: tooling that parses the message to learn which parameter failed finds a label AWS never emits and no recognisable template, so it cannot attribute the failure.
Fix: format the document-validation message in the PutLifecyclePolicy validation path from the AWS parameter label and a schema-validation phrase. This is the same defect already filed from the behavioral suite as open issue 20261022T234922Z-ecr-policy-lifecycle-document-validation-errors-carry-emulator.md, which carries no wire fixture step; this cause supplies the wire replay key so the fix can be verified against the corpus too.
Traces: api-lifecycle-policy-preview-and-apply#task#9.

Cause 3 of 3: PutSigningConfiguration names an internal Smithy list-member path instead of the API parameter.
AWS: Smithy ecr#PutSigningConfiguration validates signingProfileArn and rejects a malformed value with 400 / InvalidParameterException, message "Invalid parameter at 'signingProfileArn' failed to satisfy constraint: 'The signing profile ARN '<value>' must be well formed'" — the parameter is named by its API label and the constraint is a readable sentence that echoes the offending value.
Emulator: same status and code, but the parameter is reported as a synthetic traversal path containing the generator's list-member placeholder — "Invalid parameter at 'signingConfiguration.rules.1.member.signingProfileArn' failed to satisfy constraint: 'must satisfy regular expression '<the shape's pattern>''" — and the constraint is the raw regex rather than the documented sentence.
Why: the literal 'member' segment is an artifact of how list members are named in the model and appears in no AWS response; a client that parses the parameter path to highlight the offending field in a form or template cannot resolve it, and the raw pattern exposes model internals while dropping the value the caller actually sent.
Fix: when rendering a validation failure reached through a list or map, build the parameter label from the API's documented parameter name (dropping synthetic 'member' segments) and emit the documented constraint sentence rather than the regex, in the shared parameter-validation message formatter used by the PutSigningConfiguration handler.
Traces: ecr-put-signing-configuration-invalid-profile-arn#task#1 (representative); same cluster: ecr-put-signing-configuration-invalid-repository-filter#task#1, ecr-signing-configuration-crud-lifecycle#task#2, ecr-signing-configuration-invalid-input-then-recover#task#1 and #task#2, ecr-signing-configuration-invalid-profile-arn#task#1.

## Evidence
In all three the HTTP status is 400 on both sides and $.__type matched; only $.message diverged.
Cause 1 — fixture api-account-settings-and-authorization-token, step task#7 (GetAccountSetting): expected "Invalid parameter at 'name' failed to satisfy constraint: 'Invalid account setting name provided'", got "Invalid parameter at 'name' failed to satisfy constraint: 'Member must satisfy enum value set: [BASIC_SCAN_TYPE_VERSION, BLOB_MOUNTING, REGISTRY_POLICY_SCOPE]'". The on-disk model's GetAccountSetting.name targets AccountSettingName with length 1..64 and no enum trait, so the asserted constraint does not exist.
Cause 2 — fixture api-lifecycle-policy-preview-and-apply, step task#9 (PutLifecyclePolicy): expected "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: instance failed to match exactly one schema (matched 0 out of 4)\n'", got 'lifecyclePolicyText is not a valid lifecycle policy: rule 1 carries \'selection.countType\' "notARealCountType", which is neither "imageCountMoreThan" nor "sinceImagePushed"'. The template itself is absent from the emulator's message.
Cause 3 — fixture ecr-put-signing-configuration-invalid-profile-arn, step task#1 (PutSigningConfiguration): expected "Invalid parameter at 'signingProfileArn' failed to satisfy constraint: 'The signing profile ARN 'not-a-valid-signer-arn' must be well formed'", got "Invalid parameter at 'signingConfiguration.rules.1.member.signingProfileArn' failed to satisfy constraint: 'must satisfy regular expression 'arn:aws(-[a-z]+)*:signer:[a-z0-9-]+:[0-9]{12}:\\/signing-profiles\\/[a-zA-Z0-9_]{2,}''". Five further fixture steps in the same cluster show the identical path/constraint rendering.

## Steps to Reproduce
Replay the wire corpus and re-run the three fixture steps listed per cause. Manually, in each case compare the Message against AWS's template "Invalid parameter at '<Parameter>' failed to satisfy constraint: '<constraint>'":
1. GetAccountSetting(name='NOT_A_SETTING') -> the constraint must not quote an enum value set.
2. CreateRepository R, then PutLifecyclePolicy(repositoryName=R, lifecyclePolicyText=<rule whose selection.countType is 'notARealCountType'>) -> the message must use the template with parameter label 'LifecyclePolicyText'.
3. PutSigningConfiguration(signingConfiguration={rules:[{signingProfileArn:'not-a-valid-signer-arn', ...}]}) -> the parameter label must be 'signingProfileArn', with no 'member' segment, and the constraint must be the documented sentence rather than the raw regex.
