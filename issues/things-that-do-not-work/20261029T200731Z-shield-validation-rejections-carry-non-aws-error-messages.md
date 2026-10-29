Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: shield.create-protection-invalid-name-pattern-rejected shield.describe-protection-conflicting-params-rejected
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield.create-protection-invalid-name-pattern-rejected|run#6|AssertionError|assert bad_message == "Error validating the following inputs: name", f"unexpected message {bad_message!r}"
Issue-Key: shield.describe-protection-conflicting-params-rejected|run#6|AssertionError|assert both_message == (
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# shield: validation rejections carry non-AWS error messages (CreateProtection name pattern, DescribeProtection mutually-exclusive ids)

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProtection

## Description
Two implemented shield rejection paths return the right HTTP status and error code but the wrong message text, so clients that key on the message (the only thing separating several same-code Shield failures) diverge. 2 causes.

Cause 1 of 2: CreateProtection's Name pattern violation is reported with the generic framework constraint message instead of Shield's ValidationException wording.
AWS: Smithy CreateProtection.Name targets ProtectionName, required, @length 1..128, @pattern ^[ a-zA-Z0-9_\.\-]*$. Recorded behavior: a Name violating that pattern is refused with HTTP 400, error code `ValidationException`, message "Error validating the following inputs: name". The fixed text is "Error validating the following inputs: "; the variable part is the comma-separated list of offending input names in the service's lowercase member spelling (here `name`, taken from the request). It is the shape/pattern family, not the InvalidParameterException family — with only the Name corrected, the same request moves to the distinct `InvalidResourceException` path for the unresolvable ResourceArn.
Emulator: HTTP 400 with code `ValidationException` (correct) but the generic smithy constraint message "1 validation error detected: Value 'bad!name@@' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ a-zA-Z0-9_\\.\\-]*$" — i.e. the pre-handler constraint reporter's text, which echoes the submitted value and the raw regex.
Why: a client (or a test fixture) matching Shield's documented validation message fails to recognise the error, and the emulator leaks the request value and the model regex into a message AWS never emits — so message-based classification and redaction expectations both break.
Fix: the shield service's constraint-violation message rendering for CreateProtection (the validation layer that runs before the handler), which must emit Shield's "Error validating the following inputs: <names>" form rather than the framework default.
Traces: shield.create-protection-invalid-name-pattern-rejected.

Cause 2 of 2: DescribeProtection's mutually-exclusive-identifier rejection uses a paraphrase of the member documentation rather than Shield's error message.
AWS: Smithy DescribeProtection takes optional ProtectionId and ResourceArn and models InvalidParameterException (HTTP 400); supplying both is refused with code `InvalidParameterException` and the message "Invalid parameter. You must provide one value, either protectionId or resourceArn, but not both." — fixed text in full, naming the two members in the service's lowerCamel spelling, with no values from the request.
Emulator: code `InvalidParameterException` (correct) with the message "You must provide either the ResourceArn of the protected resource or the ProtectionID of the protection, but not both." — the Smithy member doc sentence, missing the "Invalid parameter. " prefix and using different member spellings. Related and in the same handler: the check's next leg expects a single-identifier DescribeProtection on an unsubscribed account to be refused with `ResourceNotFoundException` "The subscription does not exist.", whereas this handler answers "The protection <id> does not exist." / "The resource <arn> has no protection."; that leg is not yet reached because the message above fails first, so fixing only the conflict message will leave the check failing.
Fix: the DescribeProtection handler's InvalidParameterException construction (its invalid_parameter helper) and the same handler's not-found/subscription-gate messages.
Traces: shield.describe-protection-conflicting-params-rejected.

## Evidence
Suite report (simparity-behavioral-aws-v1), service shield:

[mismatch] shield.create-protection-invalid-name-pattern-rejected — check asserted code ValidationException (passed) then the message; AssertionError: unexpected message "1 validation error detected: Value 'bad!name@@' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[ a-zA-Z0-9_\\\\.\\\\-]*$". Expected: "Error validating the following inputs: name".

[mismatch] shield.describe-protection-conflicting-params-rejected — check asserted code InvalidParameterException (passed) then the message; AssertionError: unexpected message 'You must provide either the ResourceArn of the protected resource or the ProtectionID of the protection, but not both.'. Expected: 'Invalid parameter. You must provide one value, either protectionId or resourceArn, but not both.'.

Source: services/service-com-amazonaws-shield-awsshield-20160616/src/ops/describe_protection.rs lines 166-178 build the both-ids rejection from the member documentation sentence, and lines 219-235 (fn invalid_parameter) wrap it as InvalidParameterException; lines 186-201 build the not-found messages as "The protection {protection_id} does not exist." / "The resource {resource_arn} has no protection." instead of the account-level "The subscription does not exist." the recording shows for an unsubscribed account. The CreateProtection name message is produced by the pre-handler modeled-constraint reporter, not by create_protection.rs.

## Steps to Reproduce
Boot the emulator and point a botocore shield client at it (no Shield Advanced subscription provisioned):
1. create_protection(Name='bad!name@@', ResourceArn='arn:aws:ec2:<region>:111122223333:eip-allocation/eipalloc-00000000000000000') -> emulator: ValidationException with the generic '1 validation error detected: ...' message; AWS: ValidationException 'Error validating the following inputs: name'.
2. describe_protection(ProtectionId='12345678-1234-1234-1234-123456789012', ResourceArn='arn:aws:ec2:<region>:111122223333:eip-allocation/eipalloc-00000000000000000') -> emulator: InvalidParameterException 'You must provide either the ResourceArn of the protected resource or the ProtectionID of the protection, but not both.'; AWS: InvalidParameterException 'Invalid parameter. You must provide one value, either protectionId or resourceArn, but not both.'.
3. describe_protection(ProtectionId='12345678-1234-1234-1234-123456789012') alone -> AWS (unsubscribed account): ResourceNotFoundException 'The subscription does not exist.'; emulator: ResourceNotFoundException 'The protection 12345678-1234-1234-1234-123456789012 does not exist.'.
Re-replay shield.create-protection-invalid-name-pattern-rejected and shield.describe-protection-conflicting-params-rejected to confirm a fix.
