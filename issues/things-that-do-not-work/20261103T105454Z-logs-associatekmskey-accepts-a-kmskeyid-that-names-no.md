Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.invalid-parameter-validation-on-create
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.invalid-parameter-validation-on-create|_error_code#33|AssertionError|raise AssertionError(f"the call was expected to be refused but it succeeded, returning {result!r}")
Issue-Commit: 286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a

# logs AssociateKmsKey accepts a kmsKeyId that names no KMS key (non-ARN form) and answers 200 instead of 400 InvalidParameterException

> _Note: this issue was identified against the code base at commit `286c0019b2a6d6ba24e260ebac56f0f3f96c1e4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateKmsKey

## Description
One net-new root cause in the CloudWatch Logs emulator, grounded in the failing behavioral check logs.invalid-parameter-validation-on-create.

Cause 1 of 1: AssociateKmsKey never resolves `kmsKeyId` against KMS, so a value that is not an ARN and names no existing key is accepted and recorded on the log group.

AWS: Smithy (com.amazonaws.cloudwatchlogs#AssociateKmsKey) input = {kmsKeyId: KmsKeyId @required, length 0..256; logGroupName: LogGroupName, pattern ^[\.\-_/#A-Za-z0-9]+$; resourceIdentifier}; modeled errors InvalidParameterException (400), OperationAbortedException (400), ResourceNotFoundException (400), ServiceUnavailableException (500). The operation's own modeled documentation states the rule: "If you attempt to associate a KMS key with a log group but the KMS key does not exist or the KMS key is disabled, you receive an InvalidParameterException error." The refusal is HTTP 400, error code InvalidParameterException, message "The specified KMS Key Id could not be found." — that message is entirely fixed text for this error class (it carries no value from the request or from state; it does NOT echo the offending kmsKeyId), and the association must not be recorded, so a following DescribeLogGroups must still report no kmsKeyId for the group.

Emulator: AssociateKmsKey(logGroupName=<existing group>, kmsKeyId='not-a-real-kms-key-arn') answers HTTP 200 with an empty JSON body ({'ResponseMetadata': {'HTTPStatusCode': 200, 'content-type': 'application/x-amz-json-1.1', 'content-length': '2'}}) and stores the string as the group's kmsKeyId. The handler's only guard is crate::state::is_kms_key_id, which returns false ONLY for an empty value or for an `arn:` value whose service segment is not `kms`; any non-ARN string is deliberately passed through as "left to KMS to resolve", and nothing afterwards ever resolves it against the KMS emulator's key catalogue. On the ARN-shaped paths where that guard does fire, the refusal message is additionally emulator-authored ("kmsKeyId must be the ARN of a KMS key, got '<value>'" / "<member> must name a KMS key, got '<value>'") rather than AWS's fixed "The specified KMS Key Id could not be found.", and it echoes the request value AWS does not echo.

Why: a client that mistypes or templates a wrong key identifier (a bare id, an alias name, a placeholder left in a deployment parameter) is told the association succeeded. The log group looks CMK-encrypted while no key backs it, and the up-front refusal AWS uses to surface the misconfiguration never arrives — so the encryption defect ships and is only noticed when real log data is expected to be encrypted/decryptable.

Fix: AssociateKmsKey's input validation / key-resolution path in the logs service crate — resolve the kmsKeyId (ARN, bare key id, or alias) against the KMS emulator's keys before mutating state, refuse an unresolvable or disabled key with InvalidParameterException and AWS's fixed message, and leave the log group's kmsKeyId untouched on refusal. The refusal message helper shared by these paths must carry AWS's wording rather than the emulator-authored "must name a KMS key" template.

Related (does NOT cover this check): the open issue 20261023T113357Z-logs-cloudwatch-logs-accepts-requests-real-aws-refuses-putresourcepolicy.md describes, as its Cause 2, AssociateKmsKey/CreateLogGroup accepting invalid/malformed key ARNs, disabled CMKs and out-of-scope CMKs; its replay keys name other checks (logs.associate-kms-key-invalid-key-arn-rejected, logs.associate-kms-key-malformed-arn-rejected, kms__logs.*) and not this one, and the value here is the non-ARN form that the emulator's guard admits by design, so this check is filed separately and should be re-replayed on its own.

Traces: logs.invalid-parameter-validation-on-create (mismatch).

## Evidence
Failing check logs.invalid-parameter-validation-on-create (service heading: logs), at check_logs_invalid_parameter_validation_on_create.py line 108 -> _error_code line 23:

AssertionError: the call was expected to be refused but it succeeded, returning {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', 'date': '...'}, 'RetryAttempts': 0}}

Expected (AWS): HTTP 400, Error.Code = InvalidParameterException, Error.Message = "The specified KMS Key Id could not be found." (fixed text, no request/state values interpolated), and DescribeLogGroups(logGroupNamePrefix=<group>) must still show no kmsKeyId on the group.
Actual (emulator): HTTP 200, empty body, and the string 'not-a-real-kms-key-arn' recorded as the group's kmsKeyId.

Earlier assertions in the same check passed (CreateLogGroup with an invalid name refused InvalidParameterException with AWS's validation-error message; CreateLogStream with a colon refused; PutLogEvents to a deleted stream refused ResourceNotFoundException; Put/DeleteRetentionPolicy round trip), so the divergence is isolated to AssociateKmsKey's key resolution.

Code path: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/associate_kms_key.rs lines 225-230 guard only on crate::state::is_kms_key_id, defined at services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/state.rs lines 6750-6758, which returns true for any non-empty value that does not start with 'arn:'; the emulator-authored refusal text is state.rs lines 6742-6744 (not_a_kms_key_message).

## Steps to Reproduce
1. logs.create_log_group(logGroupName='sim2real-behav-lg-validparent-<unique>')
2. logs.associate_kms_key(logGroupName='sim2real-behav-lg-validparent-<unique>', kmsKeyId='not-a-real-kms-key-arn')
   -> emulator: HTTP 200, empty body. AWS: HTTP 400 InvalidParameterException "The specified KMS Key Id could not be found."
3. logs.describe_log_groups(logGroupNamePrefix='sim2real-behav-lg-validparent-<unique>')
   -> emulator: the group carries kmsKeyId='not-a-real-kms-key-arn'. AWS: no kmsKeyId member on the group.
4. Control that must keep working: create a KMS key whose policy allows logs.<region>.amazonaws.com, then associate_kms_key with that key ARN -> 200 and DescribeLogGroups reports kmsKeyId == that ARN; disassociate_kms_key -> kmsKeyId absent.

Replay: logs.invalid-parameter-validation-on-create
