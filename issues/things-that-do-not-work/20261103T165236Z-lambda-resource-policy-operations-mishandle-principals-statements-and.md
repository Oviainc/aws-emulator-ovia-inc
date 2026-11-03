Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-sstr-traj-apigateway-invoke-permission-forms
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-sstr-traj-apigateway-invoke-permission-forms lambda-snx-traj-alias-endpoint-qualified-policy lambda-snx-traj-policy-side-reads-and-identifier-mistakes
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|AddPermission|4xx->2xx|$.Type
Issue-Key: lambda|AddPermission|4xx->2xx|$.message
Issue-Key: lambda|AddPermission|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|AddPermission|4xx->2xx|status
Issue-Key: lambda|AddPermission|2xx->4xx|$.Statement
Issue-Key: lambda|AddPermission|2xx->4xx|status
Issue-Key: lambda|GetPolicy|2xx->2xx|$.Policy
Issue-Key: lambda|RemovePermission|4xx->4xx|$.Message
Issue-Key: lambda|GetPolicy|4xx->4xx|$.message
Issue-Key: lambda|GetPolicy|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetPolicy|4xx->4xx|status
Issue-Key: lambda|GetPolicy|4xx->4xx|$.Message
Issue-Commit: dd64cbf0b038e7955d699468def5671676941ec2

# Lambda resource-policy operations mishandle principals statements and refusals

> _Note: this issue was identified against the code base at commit `dd64cbf0b038e7955d699468def5671676941ec2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
The function resource-policy plane diverges on four fronts: AddPermission does not validate the Principal, it treats a distinct statement as a duplicate, GetPolicy returns the principal as supplied rather than as AWS stores it, and the refusal messages for missing statements, qualifier mismatches and unknown functions are all emulator-authored. 7 causes, all in the AddPermission / GetPolicy / RemovePermission handlers and the policy store.

Cause 1 of 7 - AddPermission accepts an invalid Principal.
AWS: Smithy: AddPermission input Principal is required with @pattern ^[^\\s]+$; the operation declares InvalidParameterValueException (400). A principal that is neither an account id, an IAM ARN nor a service principal of the form <service>.amazonaws.com is refused with HTTP 400, Type 'User' and the message 'The provided principal was invalid. Please check the principal and try again.' (fixed text, no variable parts).
Emulator: creates the statement and answers HTTP 201 with the Statement body.
Why: a client that abbreviates a service principal (writing the service short name instead of its domain) gets a grant that can never match a caller, and believes the permission is in place.
Fix: the AddPermission handler's Principal validation.
Traces: lambda-sstr-traj-apigateway-invoke-permission-forms#task#3.

Cause 2 of 7 - AddPermission refuses a distinct statement as a conflict.
AWS: the second grant - a different StatementId with a different SourceArn - is accepted with HTTP 201 and returns the Statement JSON (Sid, Effect, Principal, Action, Resource and the ArnLike SourceArn condition).
Emulator: answers HTTP 409 (ResourceConflictException) with no Statement.
Why: a client that grants both the control-plane and the execute-api invoke paths to API Gateway can only ever install one of them; the second is permanently rejected.
Fix: the AddPermission handler's duplicate-statement detection (it must key on StatementId, not on principal/action).
Traces: lambda-sstr-traj-apigateway-invoke-permission-forms#task#6.

Cause 3 of 7 - GetPolicy returns the principal as supplied, not the canonical service principal, and orders statements by insertion rather than as AWS does.
AWS: Smithy: GetPolicy output Policy (String) - the IAM policy document. AWS canonicalises a service principal to its full domain form, so a grant made to the service short name is stored and returned as Principal.Service='<service>.amazonaws.com'; in the recording the statements appear in the order AWS returns them, with the earlier-granted control-plane statement first.
Emulator: returns Principal.Service='<service>' - the unexpanded short name the caller wrote - and emits the statements in the reverse order.
Why: a client that reads the policy to decide whether a service is already permitted compares against the canonical domain form and concludes the grant is missing; a client diffing the policy document sees a spurious change.
Fix: the policy store's principal canonicalisation and the GetPolicy serializer's statement ordering.
Traces: lambda-sstr-traj-apigateway-invoke-permission-forms#task#7.

Cause 4 of 7 - the RemovePermission missing-statement message is emulator-authored.
AWS: Smithy: RemovePermission declares ResourceNotFoundException (404). Removing a statement id that is not in the policy is refused with HTTP 404 and the message 'Statement <statement-id> is not found in resource policy.' - fixed text with the StatementId from the request.
Emulator: answers the correct 404 but with 'The statement id (<statement-id>) provided does not exist on the policy of <function-arn>.'
Why: a client that treats AWS's sentence as idempotent-delete-succeeded does not recognise the emulator's and reports a failure.
Fix: the RemovePermission handler's not-found message.
Traces: lambda-sstr-traj-apigateway-invoke-permission-forms#task#9.

Cause 5 of 7 - the GetPolicy qualifier-mismatch message is emulator-authored.
AWS: Smithy: GetPolicy declares InvalidParameterValueException (400) and takes both a FunctionName (which may itself carry a qualifier) and a Qualifier query parameter. When the two disagree the refusal is HTTP 400 with the message 'The derived qualifier from the function name does not match the specified qualifier.' (fixed text, no variable parts).
Emulator: answers the correct 400 but with "The function name '<arn>' qualifies version '<derived>', which does not match the Qualifier parameter '<supplied>'".
Fix: the GetPolicy handler's qualifier cross-check message.
Traces: lambda-snx-traj-alias-endpoint-qualified-policy#task#12.

Cause 6 of 7 - an invalid Qualifier is answered 404 instead of a 400 validation error.
AWS: Smithy: GetPolicy input Qualifier targets NumericLatestPublishedOrAliasQualifier with @pattern ^\\$(LATEST(\\.PUBLISHED)?)|[a-zA-Z0-9-_$]+$ and @length 1..128. A value that violates the pattern is a validation failure: HTTP 400 with the message "1 validation error detected: Value '<qualifier>' at 'qualifier' failed to satisfy constraint: Member must satisfy regular expression pattern: \\$(LATEST(\\.PUBLISHED)?)|[a-zA-Z0-9-_$]+" - fixed text whose variable parts are the offending value, the member name and the pattern.
Emulator: answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and no message, i.e. it looks the malformed qualifier up as if it were a legal alias name.
Why: a client distinguishing 'you sent something invalid' (do not retry, fix the input) from 'it does not exist' (the alias may appear later) takes the wrong branch.
Fix: the GetPolicy handler's input validation, which must run before the lookup.
Traces: lambda-snx-traj-alias-endpoint-qualified-policy#task#13.

Cause 7 of 7 - the GetPolicy unknown-function message is emulator-authored.
AWS: ResourceNotFoundException (404) with the message 'The resource you requested does not exist.' (fixed text, no variable parts) when the policy's function does not exist.
Emulator: answers the correct 404 but with 'Function not found: <function-arn>'.
Why: a client matching AWS's sentence to decide the function is absent does not match, and the emulator's message also leaks the resolved ARN AWS deliberately does not echo here.
Fix: the GetPolicy handler's not-found message.
Traces: lambda-snx-traj-policy-side-reads-and-identifier-mistakes#task#11.

## Evidence
Cause 1: lambda-sstr-traj-apigateway-invoke-permission-forms#task#3 - expected 400 + $.Type='User' + $.message='The provided principal was invalid. Please check the principal and try again.'; got 201.
Cause 2: #task#6 - expected 201 with $.Statement present; got 409 with $.Statement missing.
Cause 3: #task#7 - $.Policy expected Principal.Service='apigateway.amazonaws.com' on the second statement and the control-plane statement first; got Principal.Service='apigateway' and the statements reversed.
Cause 4: #task#9 - 404 vs 404, $.Message expected 'Statement apigateway-ctl is not found in resource policy.', got 'The statement id (apigateway-ctl) provided does not exist on the policy of <arn>.'
Cause 5: lambda-snx-traj-alias-endpoint-qualified-policy#task#12 - 400 vs 400, $.message expected 'The derived qualifier from the function name does not match the specified qualifier.', got the emulator's version/qualifier sentence.
Cause 6: #task#13 - expected 400 with the 1-validation-error qualifier-pattern message; got 404 with x-amzn-errortype ResourceNotFoundException and no message.
Cause 7: lambda-snx-traj-policy-side-reads-and-identifier-mistakes#task#11 - 404 vs 404, $.Message expected 'The resource you requested does not exist.', got 'Function not found: <arn>'.

## Steps to Reproduce
1. Replay lambda-sstr-traj-apigateway-invoke-permission-forms and walk task#3 (AddPermission with an abbreviated service principal), task#6 (the second distinct grant), task#7 (GetPolicy) and task#9 (RemovePermission for an absent statement).
2. Replay lambda-snx-traj-alias-endpoint-qualified-policy task#12 and task#13 (GetPolicy with a conflicting qualifier, then with a qualifier containing a space).
3. Replay lambda-snx-traj-policy-side-reads-and-identifier-mistakes task#11 (GetPolicy on a function that does not exist).
