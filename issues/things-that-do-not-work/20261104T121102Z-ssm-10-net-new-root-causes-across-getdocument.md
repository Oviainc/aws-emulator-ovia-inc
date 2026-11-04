Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.get-aws-owned-run-shell-script-document-shape ssm.get-document-invalid-version-fails ssm.get-document-yaml-format-converts-json-content ssm.get-nonexistent-opsitem-fails ssm.delete-resource-policy-unknown-policy-id-rejected ssm.resource-policy-hash-mismatch-delete-rejected ssm.resource-policy-lifecycle ssm.put-resource-policy-malformed-document-rejected ssm.service-setting-read-semantics ssm.reset-service-setting-unknown-setting-rejected ssm.update-service-setting-unknown-id-rejected ssm.cloud-connector-full-lifecycle ssm.delete-cloud-connector-unknown-id-rejected ssm.update-association-status-invalid-document-rejected ssm.deploy-document-via-cloudformation-then-associable ssm.deploy-ssm-document-via-cloudformation-then-send-command
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.get-aws-owned-run-shell-script-document-shape|run#8|KeyError|plugins = content["runtimeConfig"]
Issue-Key: ssm.get-document-invalid-version-fails|run#17|AssertionError|assert (code, message) == (
Issue-Key: ssm.get-document-yaml-format-converts-json-content|run#12|AssertionError|assert as_yaml["Content"] == _EXPECTED_YAML, f"YAML content is {as_yaml['Content']!r}"
Issue-Key: ssm.get-nonexistent-opsitem-fails|run#10|AssertionError|assert message.startswith(
Issue-Key: ssm.delete-resource-policy-unknown-policy-id-rejected|run#17|AssertionError|assert message == (
Issue-Key: ssm.resource-policy-hash-mismatch-delete-rejected|run#19|AssertionError|assert message == (
Issue-Key: ssm.resource-policy-lifecycle|run#22|AssertionError|assert message == (
Issue-Key: ssm.put-resource-policy-malformed-document-rejected|run#10|AssertionError|assert message == "Policy is malformed and cannot be parsed", message
Issue-Key: ssm.service-setting-read-semantics|run#16|AssertionError|assert message == "", message
Issue-Key: ssm.reset-service-setting-unknown-setting-rejected|run#13|AssertionError|assert message == "", f"unexpected message {message!r}"
Issue-Key: ssm.update-service-setting-unknown-id-rejected|run#13|AssertionError|assert message == "", f"unexpected message: {message!r}"
Issue-Key: ssm.cloud-connector-full-lifecycle|_create_cloud_connector_expecting_refusal#15|AssertionError|raise AssertionError("CreateCloudConnector unexpectedly succeeded with a nonexistent role")
Issue-Key: ssm.delete-cloud-connector-unknown-id-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, call unexpectedly succeeded: {result!r}")
Issue-Key: ssm.update-association-status-invalid-document-rejected|run#7|AssertionError|assert described["Status"] == "Active", (
Issue-Key: ssm|GetDocument|InvalidDocument
Issue-Commit: 3bd56f3581a93dde5b982de16e37f58616506edc

# SSM: 10 net-new root causes across GetDocument representation, resource-policy/service-setting/OpsItem refusal messages, CreateCloudConnector validation, document status and the CloudFormation AWS::SSM::Document provider

> _Note: this issue was identified against the code base at commit `3bd56f3581a93dde5b982de16e37f58616506edc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDocument

## Description
Ten net-new root causes in the ssm crate, all grounded in failing behavioral checks listed under '### ssm'. 10 CAUSES — each must be closed before submitting.

Cause 1 of 10 — GetDocument serves the AWS-owned AWS-RunShellScript document without its plugin map.
AWS: GetDocument(Name="AWS-RunShellScript") returns an Amazon-owned Command document (Smithy: GetDocument output Status -> DocumentStatus enum, DocumentType -> DocumentType, DocumentFormat -> DocumentFormat, Content -> DocumentContent @length min 1) whose JSON Content is the schemaVersion-1.2 shape: a top-level `parameters` map containing `commands` AND a top-level `runtimeConfig` map keyed by plugin name, which contains `aws:runShellScript`. The plugin map is what makes the document runnable by Run Command.
Emulator: the document reads as Active/Command/JSON and its `parameters` do contain `commands`, but the Content object has no `runtimeConfig` key at all — the check dies with `KeyError: 'runtimeConfig'` while indexing the parsed Content.
Why: a client that introspects an AWS-owned document to discover its plugins (or that round-trips the content into its own document) sees a document body AWS never serves and cannot find the plugin it must invoke.
Fix: the AWS-owned document catalog the GetDocument/DescribeDocument handlers read from — the seeded content for AWS-RunShellScript must carry the runtimeConfig plugin map.
Traces: ssm.get-aws-owned-run-shell-script-document-shape.

Cause 2 of 10 — GetDocument with an unknown VersionName is refused with the wrong error class and an authored message.
AWS: a VersionName that no version of the document carries is a missing document, refused 400 InvalidDocument (Smithy: GetDocument errors InvalidDocument, InvalidDocumentVersion, InternalServerError; InvalidDocument httpError 400) with the message `Document with name <name> with version <versionName> does not exist.` — fixed text around the document name from the request and the requested VersionName. (InvalidDocumentVersion is AWS's class for a numeric DocumentVersion that does not exist, where the message is `Document version: <n> is invalid, latest version is <m>`; the emulator gets that numeric path right — the same check asserts it and passes.)
Emulator: 400 InvalidDocumentVersion with `Document version name: release-two is invalid` — wrong error code and emulator-authored text (the fixed text is wrong; only the version name is taken from the request).
Why: a client branching on InvalidDocument vs InvalidDocumentVersion to distinguish "no such document/version-name" from "bad numeric version" takes the wrong branch, and the message does not name the document.
Fix: the GetDocument handler's version resolution — the VersionName lookup miss must raise InvalidDocument with AWS's sentence.
Traces: ssm.get-document-invalid-version-fails.

Cause 3 of 10 — GetDocument(DocumentFormat=YAML) emits YAML that is not AWS's conversion text.
AWS: converting a JSON-authored document to YAML produces an exact text: a `---\n` document-start marker, string scalars double-quoted (`schemaVersion: "2.2"`, `description: "yaml conversion document"`, `action: "aws:runShellScript"`, `name: "run"`, `- "echo hello"`), and block sequences whose `- ` dashes sit at the parent key's indentation (`mainSteps:\n- action: ...`). Recorded behavior only for the exact serializer style; the format member itself is modeled (GetDocument.DocumentFormat -> DocumentFormat enum).
Emulator: the conversion omits the `---` marker, leaves every scalar unquoted and indents sequence dashes under their key — `schemaVersion: "2.2"\ndescription: yaml conversion document\nmainSteps:\n  - action: "aws:runShellScript"\n    name: run\n    inputs:\n      runCommand:\n        - echo hello`.
Why: a client that diffs the YAML it gets back against the YAML AWS produces (the normal way documents are reviewed and committed) sees a spurious difference on every read, and a consumer expecting quoted scalars can re-parse `2.2` as a number.
Fix: the JSON->YAML conversion in the GetDocument handler's content serializer.
Traces: ssm.get-document-yaml-format-converts-json-content.

Cause 4 of 10 — GetOpsItem's not-found message is authored prose instead of AWS's wrapped service message.
AWS: 400 OpsItemNotFoundException (Smithy: GetOpsItem errors OpsItemAccessDeniedException, OpsItemNotFoundException, InternalServerError) whose Message is the SDK-wrapped form `OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: <request id>; Proxy: null)` — the fixed text is `OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: ` and the trailing `; Proxy: null)`, with only the request id varying per call.
Emulator: 400 OpsItemNotFoundException with `The specified OpsItem ID doesn't exist. Verify the ID and try again.` — the documentation sentence, not the message AWS returns, and with no wrapped service/status/request-id suffix.
Why: a client that matches or logs AWS's message (the check matches around the request id precisely because that part is per-call) sees text AWS never emits.
Fix: the GetOpsItem handler's not-found error construction (and the shared error renderer that must append the `(Service: AmazonSSM; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)` suffix for this class).
Traces: ssm.get-nonexistent-opsitem-fails.

Cause 5 of 10 — DeleteResourcePolicy's not-found refusal message does not name the resource, policy id or hash.
AWS: deleting a (PolicyId, PolicyHash) pair that identifies no stored revision is refused 400 ResourcePolicyNotFoundException (Smithy: DeleteResourcePolicy errors ResourcePolicyNotFoundException, ResourcePolicyConflictException, ResourcePolicyInvalidParameterException, MalformedResourcePolicyDocumentException, ResourceNotFoundException) with `Resource policy for resource <resourceArn> with policy ID <policyId> and policy hash <policyHash> does not exist` — fixed text `Resource policy for resource `, ` with policy ID `, ` and policy hash `, ` does not exist`, with the ARN, id and hash taken verbatim from the request.
Emulator: the class is right but the message is `No policies with the specified policy ID and hash could be found.` — authored text naming nothing from the request.
Why: a caller that identifies which policy revision was rejected from the message (the normal way a hash-mismatch is diagnosed, since the hash changes on every write) has nothing to read.
Fix: the DeleteResourcePolicy handler's not-found error construction.
Traces: ssm.delete-resource-policy-unknown-policy-id-rejected, ssm.resource-policy-hash-mismatch-delete-rejected, ssm.resource-policy-lifecycle.

Cause 6 of 10 — PutResourcePolicy's malformed-document refusal message is reworded.
AWS: a policy body that cannot be parsed is refused 400 MalformedResourcePolicyDocumentException (Smithy: PutResourcePolicy errors MalformedResourcePolicyDocumentException, ResourcePolicyInvalidParameterException, ResourcePolicyLimitExceededException, ResourcePolicyConflictException, ResourceNotFoundException, ResourcePolicyNotFoundException) with the fixed message `Policy is malformed and cannot be parsed` (no variable parts).
Emulator: `The specified policy document is malformed or invalid.` — authored text.
Why: a client matching AWS's sentence to distinguish an unparseable body from a semantically invalid one fails.
Fix: the PutResourcePolicy handler's policy-parse failure path.
Traces: ssm.put-resource-policy-malformed-document-rejected.

Cause 7 of 10 — ServiceSettingNotFound carries an emulator-authored message where AWS sends none.
AWS: GetServiceSetting / UpdateServiceSetting / ResetServiceSetting on a SettingId SSM never provisioned are refused 400 ServiceSettingNotFound (Smithy: all three operations model ServiceSettingNotFound, httpError 400) with an EMPTY Error.Message — the error code alone carries the meaning (recorded behavior: the message member is absent/empty on the wire).
Emulator: the code is right on all three, but every one returns `The specified service setting wasn't found. Either the service name or the setting hasn't been provisioned by the Amazon Web Services service team.` — the documentation sentence shipped as the message.
Why: a client that only surfaces a message when AWS sends one now shows text AWS never sends, and anything asserting on the empty message diverges on every unprovisioned-setting call.
Fix: the service-setting handlers' not-found error construction (emit ServiceSettingNotFound with no message).
Traces: ssm.service-setting-read-semantics, ssm.reset-service-setting-unknown-setting-rejected, ssm.update-service-setting-unknown-id-rejected.

Cause 8 of 10 — CreateCloudConnector accepts requests AWS refuses: a nonexistent IAM role and an unresolvable cross-cloud configuration both succeed with 200.
AWS: CreateCloudConnector validates the RoleArn and the cross-cloud federation before minting anything. A RoleArn naming a role that does not exist (or whose trust policy lacks the ssm service principal) is refused ValidationException with `Nonexistent role or missing ssm service principal in trust policy: <roleArn>`; with a permitted role but an Azure tenant/application that cannot be federated the call is refused ValidationException with either `Unable to create Azure credentials. Please verify the Azure tenant ID, application ID, and federated identity credential configuration are correct.` or, when outbound web identity federation is off for the account, `Outbound web identity federation is disabled for this AWS account. Enable it to use cross-cloud authentication with Azure.`; a Config connector ARN whose web identity token cannot be obtained is refused ValidationException with `Unable to obtain an AWS web identity token for cross-cloud federation. Please verify the cloud connector's AWS IAM role is permitted to call sts:GetWebIdentityToken.` The only variable part of the first message is the RoleArn echoed from the request; the others are fixed. Smithy models CreateCloudConnector's RoleArn (@pattern ^arn:aws[a-z0-9-]*:iam::\d{12}:role\/[\w-\/.@+=,]{1,1017}$, required), Configuration and ConfigConnectorArn as required and binds ConflictException/ServiceQuotaExceededException/InternalServerError — the ValidationException class and these sentences are recorded behavior only.
Emulator: every one of these requests returns HTTP 200 with a freshly minted `CloudConnectorId` (e.g. `{'CloudConnectorId': '8c4b0d17-6fae-4fbb-ac39-70a9f7fa39cf'}`), so no validation happens at all and a connector that cannot work is created and listed.
Why: a client learns that a connector pointing at a nonexistent role and an unusable tenant is healthy; the refusal it must handle never arrives, and the account accumulates connectors AWS would never have created.
Fix: the CreateCloudConnector handler's input validation (resolve the RoleArn against IAM state and its trust policy, then evaluate the cross-cloud configuration) before any state is written.
Traces: ssm.cloud-connector-full-lifecycle, ssm.delete-cloud-connector-unknown-id-rejected.

Cause 9 of 10 — a document created by CreateDocument stays in Creating and is never promoted to Active.
AWS: CreateDocument returns a DocumentDescription whose Status is Creating, and the document settles to Active (Smithy: DocumentStatus enum Creating | Active | Updating | Deleting | Failed) effectively immediately, so an unpolled GetDocument/DescribeDocument right after the create reads Status Active and the document is usable as an association/Run Command target.
Emulator: GetDocument immediately after a successful CreateDocument reports Status 'Creating', and the owed transition to Active is not recorded, so a caller that does not poll never sees a usable document (the same creation path is observed settling only when a check polls, i.e. the promotion is not applied on the create).
Why: every caller that creates a document and then uses it — associations, SendCommand, CloudFormation providers — reads a document that is not yet usable and either fails or must poll where AWS does not.
Fix: the CreateDocument handler / ssm document state — record the Creating->Active transition when the document is created, the way the emulator's other owed-transition state does.
Traces: ssm.update-association-status-invalid-document-rejected.

Cause 10 of 10 — a CloudFormation-created AWS::SSM::Document is never registered in SSM, so GetDocument cannot find it.
AWS: a stack declaring AWS::SSM::Document creates the document through SSM; the physical resource id is the document name, and after the stack reaches CREATE_COMPLETE GetDocument(Name=<that name>) serves it (Status Active, the template Content) and it can be associated or sent as a command document.
Emulator: the stack reports the resource created, but GetDocument on the document name is refused 400 InvalidDocument `Document with name <name> does not exist.` — nothing was created in the SSM document store, so every post-deploy step fails on the first read.
Why: a CloudFormation-provisioned SSM document is unusable: Refs and stack outputs point at a name SSM does not know, and nothing can run or associate it. This is the AWS::SSM::Document analogue of cause 3 of the open issue 20261103T215115Z-ssm-validationexception-constraint-text-documentalreadyexists-wording-the-cloudformation.md (AWS::SSM::MaintenanceWindow physical id), which does not carry these check ids.
Fix: the CloudFormation resource provider for AWS::SSM::Document — create the document through the SSM document state and record the document name as the physical resource id.
Traces: ssm.deploy-document-via-cloudformation-then-associable, ssm.deploy-ssm-document-via-cloudformation-then-send-command.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; all 15 checks are listed under '### ssm' in the suite report. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (GetDocument errors InvalidDocument/InvalidDocumentVersion; DeleteResourcePolicy/PutResourcePolicy error sets incl. ResourcePolicyNotFoundException and MalformedResourcePolicyDocumentException; GetServiceSetting/ResetServiceSetting/UpdateServiceSetting error ServiceSettingNotFound; GetOpsItem error OpsItemNotFoundException; CreateCloudConnector required RoleArn/Configuration/ConfigConnectorArn; CreateDocument output DocumentDescription).

Cause 1 — ssm.get-aws-owned-run-shell-script-document-shape: `KeyError: 'runtimeConfig'` at `plugins = content["runtimeConfig"]`; the preceding asserts (Name/DocumentType/DocumentFormat/Status == AWS-RunShellScript/Command/JSON/Active, schemaVersion major.minor, 'commands' in parameters) all passed, so the Content is served but has no runtimeConfig key.
Cause 2 — ssm.get-document-invalid-version-fails: expected ('InvalidDocument', 'Document with name <name> with version release-two does not exist.'); got ('InvalidDocumentVersion', 'Document version name: release-two is invalid'). The DocumentVersion='99' assertion earlier in the same check passed.
Cause 3 — ssm.get-document-yaml-format-converts-json-content: expected '---\nschemaVersion: "2.2"\ndescription: "yaml conversion document"\nmainSteps:\n- action: "aws:runShellScript"\n  name: "run"\n  inputs:\n    runCommand:\n    - "echo hello"\n'; got 'schemaVersion: "2.2"\ndescription: yaml conversion document\nmainSteps:\n  - action: "aws:runShellScript"\n    name: run\n    inputs:\n      runCommand:\n        - echo hello'.
Cause 4 — ssm.get-nonexistent-opsitem-fails: GetOpsItem(OpsItemId='oi-0123456789ab') answered code OpsItemNotFoundException with message "The specified OpsItem ID doesn't exist. Verify the ID and try again."; expected a message starting 'OpsItem not found. (Service: AmazonSSM; Status Code: 400; Error Code: OpsItemNotFoundException; Request ID: ' and ending '; Proxy: null)'.
Cause 5 — ssm.resource-policy-lifecycle / ssm.resource-policy-hash-mismatch-delete-rejected / ssm.delete-resource-policy-unknown-policy-id-rejected: code ResourcePolicyNotFoundException matched; message was 'No policies with the specified policy ID and hash could be found.' where AWS sends 'Resource policy for resource <arn> with policy ID <id> and policy hash <hash> does not exist'.
Cause 6 — ssm.put-resource-policy-malformed-document-rejected: expected message 'Policy is malformed and cannot be parsed'; got 'The specified policy document is malformed or invalid.'
Cause 7 — ssm.service-setting-read-semantics / reset-service-setting-unknown-setting-rejected / update-service-setting-unknown-id-rejected: code ServiceSettingNotFound matched; message was "The specified service setting wasn't found. Either the service name or the setting hasn't been provisioned by the Amazon Web Services service team." where AWS sends an empty message. In the read check the preceding GetServiceSetting asserts (SettingId, ARN arn:aws:ssm:<region>:<account>:servicesetting<settingId>, Status, SettingValue, LastModifiedDate, LastModifiedUser) all passed.
Cause 8 — ssm.cloud-connector-full-lifecycle: AssertionError 'CreateCloudConnector unexpectedly succeeded with a nonexistent role'. ssm.delete-cloud-connector-unknown-id-rejected: 'expected a ClientError, call unexpectedly succeeded: {'CloudConnectorId': '8c4b0d17-6fae-4fbb-ac39-70a9f7fa39cf', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}'.
Cause 9 — ssm.update-association-status-invalid-document-rejected: "expected newly-created document 'sim2real-ssm-doc-<id>' to be Active, got 'Creating'" on GetDocument immediately after CreateDocument.
Cause 10 — ssm.deploy-document-via-cloudformation-then-associable and ssm.deploy-ssm-document-via-cloudformation-then-send-command: uncaught botocore.errorfactory.InvalidDocument on GetDocument — 'Document with name sim2real-cfn-doc-<id> does not exist.' / 'Document with name sim2real-ssmdoc-<id> does not exist.' after the stack reported the AWS::SSM::Document resource created.

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids. 3. Per cause: GetDocument(Name='AWS-RunShellScript') and inspect the parsed Content for runtimeConfig (cause 1); CreateDocument with VersionName='release-one' then GetDocument(Name=<name>, VersionName='release-two') (cause 2); CreateDocument with JSON content then GetDocument(Name=<name>, DocumentFormat='YAML') and compare the text byte for byte (cause 3); GetOpsItem(OpsItemId='oi-0123456789ab') (cause 4); PutResourcePolicy on a parameter ARN, DeleteResourcePolicy with the returned (PolicyId, PolicyHash), then repeat the identical delete (cause 5); PutResourcePolicy with an unparseable Policy body (cause 6); GetServiceSetting / UpdateServiceSetting / ResetServiceSetting with SettingId 'arn:aws:ssm:<region>:<account>:servicesetting/ssm/parameter-store/does-not-exist' and read Error.Message (cause 7); CreateCloudConnector with a RoleArn naming a role that was never created, then with a real ssm-trusted role and an unresolvable Azure tenant/application (cause 8); CreateDocument then GetDocument immediately, without polling, and read Status (cause 9); CreateStack with a template declaring AWS::SSM::Document, wait for CREATE_COMPLETE, then GetDocument(Name=<the document name>) (cause 10).
