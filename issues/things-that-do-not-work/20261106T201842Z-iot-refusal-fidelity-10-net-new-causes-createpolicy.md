Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.iot-policy-invalid-condition-operator-rejected iot.iot-policy-out-of-range-source-ip-cidr-rejected iot.op-create-policy-malformed-document-rejected iot.iot-duplicate-policy-name-rejected-without-overwriting-document iot.authorizer-lifecycle-and-active-delete-guard iot.describe-endpoint-types iot.create-topic-rule-invalid-sql-fails iot.topic-rule-invalid-sql-rejected iot.command-mandatory-parameters-empty-rejected iot.delete-package-with-existing-versions-rejected iot.delete-package-with-versions-conflict-rejected iot.create-package-version-conflict-duplicate-rejected iot.package-version-duplicate-conflict-rejected iot.package-version-duplicate-name-rejected iot.package-version-update-conditional-conflict iot.deploy-provisioning-template-cfn-issues-claim-certificates iot.deploy-role-alias-cfn-enforces-session-duration
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.iot-policy-invalid-condition-operator-rejected|_error#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.iot-policy-out-of-range-source-ip-cidr-rejected|_error_code#3|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: iot.op-create-policy-malformed-document-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.iot-duplicate-policy-name-rejected-without-overwriting-document|run#9|AssertionError|assert message == expected_message, (
Issue-Key: iot.authorizer-lifecycle-and-active-delete-guard|_err#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.describe-endpoint-types|_error_code#8|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.create-topic-rule-invalid-sql-fails|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iot.topic-rule-invalid-sql-rejected|_error_code_with_retry#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: iot.command-mandatory-parameters-empty-rejected|run#6|AssertionError|assert err == ("ValidationException", (
Issue-Key: iot.delete-package-with-existing-versions-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iot.delete-package-with-versions-conflict-rejected|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iot.create-package-version-conflict-duplicate-rejected|run#11|AssertionError|assert err == ("ConflictException", (
Issue-Key: iot.package-version-duplicate-conflict-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.package-version-duplicate-name-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot.package-version-update-conditional-conflict|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iot|CreateStack|ValidationError
Issue-Commit: 9ad1a007c88406c445bb3f2d55ab006d23610bba

# iot refusal fidelity: 10 net-new causes — CreatePolicy/CreateTopicRule/DescribeEndpoint/DeleteAuthorizer accept what AWS refuses, and policy/package/command refusal messages are emulator-authored; CFN rejects two AWS::IoT resource types

> _Note: this issue was identified against the code base at commit `9ad1a007c88406c445bb3f2d55ab006d23610bba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Ten net-new root causes across the IoT policy, authorizer, endpoint, topic-rule, command and package surfaces, plus CloudFormation coverage of two AWS::IoT resource types. All were found under the `iot` service heading. Each cause is independent.

Cause 1 of 10: CreatePolicy performs no semantic validation of the policy document.
AWS: Smithy CreatePolicy models MalformedPolicyException (400). A document whose Condition uses an unknown operator is refused MalformedPolicyException with the fixed template `Policy document is Malformed: Invalid Condition type : <operator>` (<operator> is the rejected operator from the request); a document whose `IpAddress`/`aws:SourceIp` value is an out-of-range CIDR is refused MalformedPolicyException `Policy document is Malformed: Invalid IP address in Conditions`. In both cases nothing is stored (GetPolicy then refuses ResourceNotFoundException `Policy default version not found`).
Emulator: both documents are accepted with HTTP 200 and stored verbatim (policyVersionId '1').
Why: over-permissive — a policy real AWS rejects is created here, so misconfigured authorization policies pass the emulator and fail in production.
Fix: policy-document validation in CreatePolicy (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_policy.rs).
Traces: iot.iot-policy-invalid-condition-operator-rejected, iot.iot-policy-out-of-range-source-ip-cidr-rejected.

Cause 2 of 10: CreatePolicy with a non-JSON policyDocument answers an emulator-authored parser message.
AWS: MalformedPolicyException with the fixed sentence `Policy document is Malformed: Invalid policy syntax.` — no parser detail, no offsets.
Emulator: `policyDocument is not a valid JSON document: expected ident at line 1 column 2` — a serde parser message, leaking line/column detail AWS never returns.
Why: the message is not AWS's and exposes internal parser wording.
Fix: the malformed-document refusal in CreatePolicy (create_policy.rs).
Traces: iot.op-create-policy-malformed-document-rejected.

Cause 3 of 10: CreatePolicy duplicate-name refusal message.
AWS: ResourceAlreadyExistsException with the fixed template `Policy cannot be created - name already exists (name=<policyName>)`.
Emulator: `Policy <policyName> already exists`.
Why: wording clients assert on is wrong.
Fix: CreatePolicy's conflict refusal (create_policy.rs).
Traces: iot.iot-duplicate-policy-name-rejected-without-overwriting-document.

Cause 4 of 10: DeleteAuthorizer deletes an ACTIVE authorizer instead of refusing.
AWS: Smithy DeleteAuthorizer models InvalidRequestException (400) and DeleteConflictException (409). An authorizer in ACTIVE status cannot be deleted; the refusal is InvalidRequestException with the fixed template `Cannot delete authorizer <authorizerName> in ACTIVE status. Update status to INACTIVE and delete.`, and the authorizer is still readable and still ACTIVE afterwards.
Emulator: DeleteAuthorizer on an ACTIVE authorizer succeeds with HTTP 200 (empty body).
Why: over-permissive — the UpdateAuthorizer(status=INACTIVE)-then-delete contract is unenforced, so a destructive call that AWS blocks goes through.
Fix: the ACTIVE-status guard in DeleteAuthorizer (delete_authorizer.rs).
Traces: iot.authorizer-lifecycle-and-active-delete-guard.

Cause 5 of 10: DescribeEndpoint accepts retired and unknown endpointType values.
AWS: Smithy DescribeEndpoint models InvalidRequestException (400). `iot:Jobs` is retired and refused with the fixed sentence `IoT Jobs and Commands APIs are now available through iot:Data-ATS endpoints instead of iot:Jobs endpoints. Please use iot:Data-ATS.`; an unrecognized value is refused InvalidRequestException `Endpoint type <endpointType> not recognized.` (<endpointType> echoed from the request).
Emulator: DescribeEndpoint(endpointType='iot:Jobs') answers HTTP 200 with a `…jobs.iot.<region>.amazonaws.com` address, and an unknown type is likewise not refused.
Why: clients are handed an endpoint host for a type AWS no longer serves, hiding a migration that real AWS forces.
Fix: endpointType validation in DescribeEndpoint (describe_endpoint.rs).
Traces: iot.describe-endpoint-types.

Cause 6 of 10: CreateTopicRule accepts invalid rule SQL.
AWS: Smithy CreateTopicRule models SqlParseException (400). A payload whose `sql` is not a valid rule statement is refused SqlParseException whose message is the parser's fixed form `Expected 'SELECT'\n\n<sql>\n^ at 1:1` (the echoed statement and the caret position come from the request), and no rule is created (the name is absent from ListTopicRules).
Emulator: CreateTopicRule with `SELEKT *** FROM WHERE THIS IS NOT VALID SQL !!` is accepted with HTTP 200 and the rule is created.
Why: over-permissive — a rule that can never run is created, and no client ever sees SqlParseException.
Fix: SQL parsing/validation in CreateTopicRule (create_topic_rule.rs; the same validation is needed by ReplaceTopicRule).
Traces: iot.create-topic-rule-invalid-sql-fails, iot.topic-rule-invalid-sql-rejected.

Cause 7 of 10: CreateCommand's empty-mandatoryParameters ValidationException renders the value wrongly.
AWS: Smithy CreateCommand.mandatoryParameters is @length(min: 1) and the operation models ValidationException (400). An empty list is refused with the fixed template `1 validation error detected: Value '[]' at 'mandatoryParameters' failed to satisfy constraint: Member must have length greater than or equal to 1` — the rejected value is rendered as `[]`.
Emulator: same code and constraint sentence, but renders the value as `'0 items'`.
Why: the message differs from AWS's for every length-constraint refusal rendered this way, so message assertions fail and the rejected value is not shown.
Fix: the constraint-violation value rendering used by CreateCommand's validation (create_command.rs / shared validation formatter).
Traces: iot.command-mandatory-parameters-empty-rejected.

Cause 8 of 10: DeletePackage's "versions still exist" refusal message.
AWS: the refusal's message is the fixed template `Unable to delete package '<packageName>' as there are package versions associated with it` (package name quoted, from the request).
Emulator: `Package <packageName> still holds 1 version(s); all package versions must be deleted before deleting the software package` — emulator prose that also leaks a version count AWS does not include.
Why: wording is not AWS's; the error code already matches, so only the message diverges.
Fix: DeletePackage's refusal message (delete_package.rs).
Traces: iot.delete-package-with-existing-versions-rejected, iot.delete-package-with-versions-conflict-rejected.

Cause 9 of 10: CreatePackageVersion duplicate-version ConflictException message.
AWS: Smithy CreatePackageVersion models ConflictException (409); the duplicate refusal is the fixed template `Conditional check failure: Received duplicate request for package <packageName> version <versionName>`.
Emulator: `Version <versionName> of package <packageName> already exists`.
Why: wording is not AWS's; it also drops the "Conditional check failure" prefix clients key idempotency handling on. Note: open issue 20261105T035750Z-iot-createpackageversion-duplicate-version-conflictexception-message-is-emulator-authored.md describes the same wording for a different check and does not carry these four checks' replay keys.
Fix: CreatePackageVersion's conflict refusal (create_package_version.rs).
Traces: iot.create-package-version-conflict-duplicate-rejected, iot.package-version-duplicate-conflict-rejected, iot.package-version-duplicate-name-rejected, iot.package-version-update-conditional-conflict.

Cause 10 of 10: CloudFormation does not recognize AWS::IoT::ProvisioningTemplate or AWS::IoT::RoleAlias.
AWS: both resource types are supported by CloudFormation; a stack declaring them is created (CREATE_COMPLETE) and the resources are then readable through DescribeProvisioningTemplate / DescribeRoleAlias, with Ref/GetAtt resolving to the resource's name/ARN.
Emulator: CreateStack is refused ValidationError `Template format error: Unrecognized resource types: [AWS::IoT::ProvisioningTemplate]` (and `[AWS::IoT::RoleAlias]`), so no IoT provisioning template or role alias can be deployed by template at all.
Why: template-driven IoT provisioning cannot be emulated; every stack carrying these types fails at create. Related but distinct from open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md, which names AWS::IoT::Authorizer and does not carry these two checks.
Fix: register the AWS::IoT::ProvisioningTemplate and AWS::IoT::RoleAlias resource providers in the CloudFormation resource-type registry and map them onto the iot handlers.
Traces: iot.deploy-provisioning-template-cfn-issues-claim-certificates, iot.deploy-role-alias-cfn-enforces-session-duration.

## Evidence
Expected vs actual, per cause (replayed suite output):
1. CreatePolicy(Condition={'StringEqualsNope': …}) — expected MalformedPolicyException 'Policy document is Malformed: Invalid Condition type : StringEqualsNope'; actual HTTP 200 with policyVersionId '1' and the document stored. CreatePolicy with an out-of-range aws:SourceIp CIDR — expected MalformedPolicyException 'Policy document is Malformed: Invalid IP address in Conditions'; actual success.
2. Malformed JSON document — expected 'Policy document is Malformed: Invalid policy syntax.'; actual 'policyDocument is not a valid JSON document: expected ident at line 1 column 2'.
3. Duplicate policy name — expected 'Policy cannot be created - name already exists (name=<name>)'; actual 'Policy <name> already exists'.
4. DeleteAuthorizer on ACTIVE — expected InvalidRequestException 'Cannot delete authorizer <name> in ACTIVE status. Update status to INACTIVE and delete.'; actual HTTP 200 (content-length 2).
5. DescribeEndpoint(endpointType='iot:Jobs') — expected InvalidRequestException with the iot:Data-ATS migration sentence; actual HTTP 200 {'endpointAddress': '<id>.jobs.iot.<region>.amazonaws.com'}.
6. CreateTopicRule with invalid sql — expected SqlParseException "Expected 'SELECT'\n\n<sql>\n^ at 1:1"; actual HTTP 200, rule created.
7. CreateCommand(mandatoryParameters=[]) — expected "1 validation error detected: Value '[]' at 'mandatoryParameters' failed to satisfy constraint: Member must have length greater than or equal to 1"; actual the same sentence with "Value '0 items'".
8. DeletePackage with a live version — expected "Unable to delete package '<pkg>' as there are package versions associated with it"; actual 'Package <pkg> still holds 1 version(s); all package versions must be deleted before deleting the software package'.
9. CreatePackageVersion duplicate — expected 'Conditional check failure: Received duplicate request for package <pkg> version <ver>'; actual 'Version <ver> of package <pkg> already exists'.
10. CreateStack with AWS::IoT::ProvisioningTemplate / AWS::IoT::RoleAlias — expected CREATE_COMPLETE; actual ValidationError 'Template format error: Unrecognized resource types: [AWS::IoT::ProvisioningTemplate]' / '[AWS::IoT::RoleAlias]'.

## Steps to Reproduce
Replay the listed checks (suite simparity-behavioral-aws-v1):
1. CreatePolicy with Condition operator 'StringEqualsNope'; CreatePolicy with Condition IpAddress/aws:SourceIp set to an out-of-range CIDR.
2. CreatePolicy with a policyDocument that is not JSON.
3. CreatePolicy(name, doc) twice.
4. CreateAuthorizer(status='ACTIVE'); DeleteAuthorizer(name).
5. DescribeEndpoint(endpointType='iot:Jobs') and DescribeEndpoint(endpointType='iot:NotAnEndpointType').
6. CreateTopicRule(ruleName=r, topicRulePayload={'sql': 'SELEKT *** FROM WHERE THIS IS NOT VALID SQL !!', 'actions': [...]}) then ListTopicRules.
7. CreateCommand(commandId=c, mandatoryParameters=[]) with botocore parameter validation disabled.
8. CreatePackage + CreatePackageVersion, then DeletePackage.
9. CreatePackageVersion(pkg, 'v1') twice.
10. CreateStack with a template declaring AWS::IoT::ProvisioningTemplate, and one declaring AWS::IoT::RoleAlias.
