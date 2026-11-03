Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: inspector2.update-encryption-key-invalid-arn-pattern-rejected
Issue-Shape-Id: com.amazonaws.inspector2#Inspector2
Issue-Key: inspector2.update-encryption-key-invalid-arn-pattern-rejected|run#16|AssertionError|assert msg == _BAD_ARN_MESSAGE, msg
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# inspector2 UpdateEncryptionKey pattern-violation ValidationException message interpolates the rejected value and quotes the @pattern with its anchors

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEncryptionKey

## Description
ONE cause (1 of 1): the emulator's pattern-constraint message builder for inspector2 ValidationException renders a different sentence than AWS on UpdateEncryptionKey. The HTTP status (400) and the error code (ValidationException) are correct; only the `message` string diverges, in two consistent ways — (a) the emulator interpolates the REJECTED VALUE into the sentence ("Value '<value>' at 'kmsKeyId' ...") where AWS writes "Value at 'kmsKeyId' ..." for this constraint class, and (b) the emulator quotes the member's regular expression WITH its leading `^` and trailing `$`, where AWS strips both anchors from the pattern it quotes.

Cause 1 of 1: UpdateEncryptionKey with a kmsKeyId that does not match the member's @pattern.
AWS: Smithy: UpdateEncryptionKey (PUT /encryptionkey/update) input kmsKeyId (required, target KmsKeyArn, @pattern "^arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))$"); modeled error ValidationException (httpError 400). Recorded behavior: 400 ValidationException with message "1 validation error detected: Value at 'kmsKeyId' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))" — the rejected value is ABSENT and the quoted pattern carries NO `^` and NO `$`. The recording also shows shape validation precedes the enablement/state check: the same refusal is produced both while LAMBDA_CODE scanning is DISABLED for the account and while it is ENABLED with a real customer key configured, and the rejected call leaves the configured key untouched.
Fixed vs variable text: the fixed text is "<N> validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". The variable parts are the error count, the member path (`kmsKeyId`, from the request's shape) and the pattern (from the model, anchors stripped). The rejected value is NOT part of the template — never interpolate it.
Emulator: answers 400 ValidationException but with "1 validation error detected: Value 'not-a-valid-kms-arn' at 'kmsKeyId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))$" — rejected value interpolated, pattern quoted with its anchors. ('not-a-valid-kms-arn' is the one value the check happened to submit; the defect is value-independent.)
Why: a client or harness that matches AWS's pattern-violation wording for a malformed KMS key ARN never matches, and because the submitted value is echoed the message changes for every distinct bad input, so the refusal is not stable across requests the way AWS's is.
Fix: the request-validation path of UpdateEncryptionKey and the shared constraint-violation message builder it calls (pattern branch): suppress the rejected value and quote the model's @pattern with its leading `^` and trailing `$` removed. Related open issue 20261029T171819Z-inspector2-constraint-violation-validationexception-messages-add-the-rejected.md reports the same builder defect reached from CreateFilter / CreateConnector / CreateCodeSecurityScanConfiguration; it does NOT name UpdateEncryptionKey, so this path is still refused with the wrong text. Note also the open stub issue 20261029T202306Z-inspector2-updateencryptionkey-stub.md — validation now runs ahead of that stub, so the message text is a separate defect from the unimplemented happy path.
Traces: behavioral check inspector2.update-encryption-key-invalid-arn-pattern-rejected.

## Evidence
Behavioral check inspector2.update-encryption-key-invalid-arn-pattern-rejected, assertion at line 75 (`assert msg == _BAD_ARN_MESSAGE`):

expected message: "1 validation error detected: Value at 'kmsKeyId' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))"

actual message: "1 validation error detected: Value 'not-a-valid-kms-arn' at 'kmsKeyId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?:key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))$"

HTTP status 400 and error code ValidationException agree with AWS; the two text deltas are the interpolated "'not-a-valid-kms-arn' " segment after "Value " and the `^`/`$` anchors around the quoted pattern.

## Steps to Reproduce
1. Point a client at the emulator's inspector2 endpoint.
2. Call UpdateEncryptionKey (PUT /encryptionkey/update) with scanType=CODE, resourceType=AWS_LAMBDA_FUNCTION and kmsKeyId set to any string that does not match the member's @pattern (e.g. a non-ARN string).
3. Observe 400 ValidationException. Compare the `message` to AWS's: AWS omits the rejected value ("Value at 'kmsKeyId' ...") and quotes the pattern without its leading `^` and trailing `$`; the emulator interpolates the value and keeps the anchors.
4. Repeat with LAMBDA_CODE scanning disabled for the account and again with it enabled and a well-formed customer key configured — AWS produces the same shape-validation refusal in both states and leaves the configured key unchanged.
5. Equivalent replay: run behavioral check inspector2.update-encryption-key-invalid-arn-pattern-rejected.
