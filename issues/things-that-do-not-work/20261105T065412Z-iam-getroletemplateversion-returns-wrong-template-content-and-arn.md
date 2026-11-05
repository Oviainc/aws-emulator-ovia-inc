Issue-Source: trace-sweep-divergence
Issue-Trace: iam-get-role-template-version-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-get-role-template-version-error-paths iam-get-role-template-version-lookup iam-service-access-analysis-and-role-template-reports
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetRoleTemplateVersion|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.AssumeRolePolicyDocumentTemplate
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.Description
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.ParametersDefinition.member[*].DefaultValue
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.ParametersDefinition.member[*].Description
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.ParametersDefinition.member[*].Immutable
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.ParametersDefinition.member[*].IsRequired
Issue-Key: iam|GetRoleTemplateVersion|2xx->2xx|$.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion.TemplateVersionId
Issue-Key: iam|GetRoleTemplateVersion|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|GetRoleTemplateVersion|4xx->4xx|status
Issue-Commit: 9a82b60038c53263903336e1e70cc5d0e66ab9f9

# IAM GetRoleTemplateVersion returns wrong template content and ARN errors

> _Note: this issue was identified against the code base at commit `9a82b60038c53263903336e1e70cc5d0e66ab9f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoleTemplateVersion

## Description
Three distinct causes in the IAM GetRoleTemplateVersion path: the ARN of an unknown template produces the wrong NoSuchEntity message, a syntactically invalid template ARN is not validated at all, and the AWS-published role template version this emulator serves does not carry AWS's content. All three are the same operation and the same service (iam), and all must be closed before this issue is submitted.

Cause 1 of 3: NoSuchEntity message wording for a well-formed but unknown role-template ARN.
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model) binds GetRoleTemplateVersion with input GetRoleTemplateVersionRequest{TemplateArn @required arnType @length 20..2048, MinorVersion minorVersionType} and errors NoSuchEntityException (awsQueryError NoSuchEntity, httpError 404) / InvalidInputException (InvalidInput, 400) / ServiceFailureException (500). NoSuchEntityException is documented as "referenced a resource entity that does not exist. The error message describes the resource." The recording shows the message for this error class as the fixed text "Role template not found: " followed by one variable part: the TemplateArn exactly as supplied in the request (in the recording an arn:{partition}:iam::aws:role-template/{servicePrincipal}/{templateName}:{majorVersion} value). Status 404 and code NoSuchEntity are correct in the emulator.
Emulator: answers 404/NoSuchEntity but with the message "Role Template <TemplateArn> cannot be found." — different fixed text ("Role Template " + " cannot be found." instead of the "Role template not found: " prefix) around the same request-supplied ARN.
Why: a client that branches on, logs or asserts the IAM not-found message text for role templates sees wording AWS never emits; error-message assertions and operator-facing diagnostics that key off AWS's phrasing break even though the status and code line up.
Fix: the error construction in the GetRoleTemplateVersion handler's template-lookup miss branch (the NoSuchEntity message template), not its status/code mapping, which is already right.
Traces: iam-get-role-template-version-error-paths#task#2.

Cause 2 of 3: a malformed role-template ARN is answered 404 NoSuchEntity instead of a 400 validation error.
AWS: the same modeled error set gives InvalidInputException/400 for "an invalid or out-of-range value ... supplied for an input parameter", and the recording answers this request 400 with Error.Code "ValidationError" (recorded behavior only for the code string; the model's awsQueryError for InvalidInputException is "InvalidInput"). The recorded message is entirely fixed text with no request-derived part: "The specified role template ARN is not valid. Expected format is: arn:{partition}:iam::aws:role-template/{servicePrincipal}/{templateName}:{majorVersion} (majorVersion must be a positive integer with no leading zeros)." — i.e. AWS rejects a TemplateArn that does not match that shape BEFORE looking it up, and the braced tokens are literal placeholder text in the message, not substituted values.
Emulator: skips the shape check and goes straight to the catalog lookup, so a TemplateArn with no /{servicePrincipal}/{templateName}:{majorVersion} tail (the recording's value has only a single trailing segment) comes back 404 with Code "NoSuchEntity" and message "Role Template <TemplateArn> cannot be found.".
Why: a client cannot distinguish "you built the ARN wrong" from "that template does not exist": a programming error is reported as a retriable/absent-resource condition on the wrong HTTP status (404 vs 400), so SDK retry and error-classification logic takes the wrong branch and the caller is never told the ARN format rule it violated.
Fix: input validation for GetRoleTemplateVersion — a TemplateArn syntax check ahead of the state lookup, raising the 400 ValidationError with the documented format message.
Traces: iam-service-access-analysis-and-role-template-reports#task#8.

Cause 3 of 3: the AWS-published role template version is served with emulator-invented content and a non-AWS version identifier.
AWS: GetRoleTemplateVersion returns RoleTemplateVersion @required. Smithy constrains RoleTemplateVersion.TemplateVersionId to idType @length 16..128 @pattern ^[\w]+$ — AWS's IAM unique-id format: a fixed uppercase type prefix (the recording's value uses the role-template-version prefix AVTA) followed by uppercase base32 characters, opaque and carrying no human-readable text. ParametersDefinition is a list of ParameterDefinition{Name @required, Type @required, SubType, Description, IsRequired default false, DefaultValue, Immutable default false}; the recording shows the AWS-published template's parameter definitions with IsRequired and Immutable false and AWS's own parameter Description text, and shows Description / AssumeRolePolicyDocumentTemplate carrying AWS's published prose and its pretty-printed (CRLF- and space-indented) JSON trust-policy template. Recorded behavior only for the specific published wording.
Emulator: status 200 and the right shape, but the catalog entry it projects is a local invention: TemplateVersionId is a lowercase underscore-joined slug ("roletemplateversion_<...>" in the replay) that is not in IAM's unique-id format at all; Description is a one-line paraphrase instead of the published description; AssumeRolePolicyDocumentTemplate is minified JSON rather than the published pretty-printed document; and the ParameterDefinition members invert the published flags (Immutable and IsRequired reported true where AWS reports false), with substituted Description and DefaultValue values.
Why: the published template catalog is the contract a caller reads before AcquireRole — it decides which parameters it must supply (IsRequired), which it may later change (Immutable), and which version id to pin. Inverted flags make optional parameters look mandatory and mutable parameters look frozen, and a version id outside IAM's unique-id format fails any client that validates or persists it, so template-driven role provisioning is configured from wrong metadata.
Fix: the role-template catalog state (the seeded published template versions and their parameter definitions) plus the GetRoleTemplateVersion serializer that projects it — the version id must be minted in IAM's unique-id format rather than derived from the template's name.
Traces: iam-get-role-template-version-lookup#task#1, iam-get-role-template-version-lookup#task#2.

Related open issue 20261103T132715Z-iam-acquirerole-answers-404-so-no-role-template.md covers AcquireRole against the template catalog; it carries no replay key for any GetRoleTemplateVersion step and does not cover these three causes.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 79.1%, emulator_up true, bootstrap_signal false):

Cause 1 — iam-get-role-template-version-error-paths#task#2, GetRoleTemplateVersion, verdict mismatch, status expected 404 / got 404: $.ErrorResponse.Error.Message value_mismatch — expected 'Role template not found: arn:aws:iam::aws:role-template/iam.amazonaws.com/PowerUserRoleTemplate:1', got 'Role Template arn:aws:iam::aws:role-template/iam.amazonaws.com/PowerUserRoleTemplate:1 cannot be found.' (the ARN is the request's TemplateArn in both; only the fixed text differs).

Cause 2 — iam-service-access-analysis-and-role-template-reports#task#8, GetRoleTemplateVersion, verdict mismatch, status expected 400 / got 404: $.ErrorResponse.Error.Code expected 'ValidationError', got 'NoSuchEntity'; $.ErrorResponse.Error.Message expected 'The specified role template ARN is not valid. Expected format is: arn:{partition}:iam::aws:role-template/{servicePrincipal}/{templateName}:{majorVersion} (majorVersion must be a positive integer with no leading zeros).', got 'Role Template arn:aws:iam::aws:role-template/nonexistent-role-template cannot be found.'.

Cause 3 — iam-get-role-template-version-lookup#task#1 (cluster of 2, also #task#2), GetRoleTemplateVersion, verdict mismatch, status expected 200 / got 200, diverging members under $.GetRoleTemplateVersionResponse.GetRoleTemplateVersionResult.RoleTemplateVersion: TemplateVersionId (expected an IAM unique id of the AVTA… form, got 'roletemplateversion_iam_power_user_1_0'); Description (expected the published multi-sentence description of the PowerUser role template, got 'Power user role for the service principal that you specify'); AssumeRolePolicyDocumentTemplate (expected the published CRLF/pretty-printed trust-policy JSON, got the same policy minified); ParametersDefinition.member[0].Immutable expected 'false' got 'true'; member[0].Description expected 'Service principal of the service' got 'The service principal that may assume the role'; member[1].IsRequired expected 'false' got 'true'; member[1].Immutable expected 'false' got 'true'; member[1].Description and member[1].DefaultValue likewise diverge.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it (/__admin/health answers 200; the data listener accepts).
2. Replay fixture iam-get-role-template-version-error-paths step task#2 — GetRoleTemplateVersion with a well-formed role-template ARN that does not resolve — and observe 404/NoSuchEntity carrying 'Role Template <arn> cannot be found.' instead of 'Role template not found: <arn>'.
3. Replay fixture iam-service-access-analysis-and-role-template-reports step task#8 — GetRoleTemplateVersion with a TemplateArn that does not match arn:{partition}:iam::aws:role-template/{servicePrincipal}/{templateName}:{majorVersion} — and observe 404 'NoSuchEntity' instead of 400 'ValidationError' with the ARN-format message.
4. Replay fixture iam-get-role-template-version-lookup steps task#1 and task#2 — GetRoleTemplateVersion for a published role template version — and compare RoleTemplateVersion.TemplateVersionId, Description, AssumeRolePolicyDocumentTemplate and every ParametersDefinition member's Description/DefaultValue/IsRequired/Immutable against the recorded response.
