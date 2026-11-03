Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.account-policy-lifecycle logs.account-policy-lifecycle-transitions logs.delete-account-policy-nonexistent-rejected logs.delete-nonexistent-account-policy-fails logs.put-account-policy-concurrent-update-no-corruption logs.put-account-policy-then-describe logs.account-policy-limit-exceeded-enforced logs.account-policy-malformed-document-rejected logs.create-log-group-invalid-name-rejected logs.create-log-stream-invalid-name-rejected logs.invalid-parameter-validation-on-create logs.list-validation-error-semantics logs.create-log-group-name-exceeds-length-limit-rejected logs.create-log-group-name-exceeds-max-length-rejected logs.delete-integration-unknown-name-rejected logs.get-integration-nonexistent-fails logs.delete-log-anomaly-detector-nonexistent-fails logs.get-deleted-log-anomaly-detector-fails logs.get-log-anomaly-detector-nonexistent-fails logs.delete-transformer-nonexistent-fails logs.get-data-protection-policy-nonexistent-log-group-rejected logs.create-scheduled-query-duplicate-name-conflict-fails
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.account-policy-lifecycle|run#21|AssertionError|assert message == expected_message, (
Issue-Key: logs.account-policy-lifecycle-transitions|run#14|AssertionError|assert message == expected_message, (
Issue-Key: logs.delete-account-policy-nonexistent-rejected|run#12|AssertionError|assert message == "Specified resource does not exist!", (
Issue-Key: logs.delete-nonexistent-account-policy-fails|run#7|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: logs.put-account-policy-concurrent-update-no-corruption|run#19|AssertionError|assert msg == "Specified resource does not exists!", f"got {msg!r}"
Issue-Key: logs.put-account-policy-then-describe|run#15|AssertionError|assert msg == "Specified resource does not exists!", f"got {msg!r}"
Issue-Key: logs.account-policy-limit-exceeded-enforced|run#14|AssertionError|assert message == expected_message, (
Issue-Key: logs.account-policy-malformed-document-rejected|run#15|AssertionError|assert message == expected_message, (
Issue-Key: logs.create-log-group-invalid-name-rejected|run#6|AssertionError|assert msg == (
Issue-Key: logs.create-log-stream-invalid-name-rejected|run#9|AssertionError|assert msg == (
Issue-Key: logs.create-log-group-name-exceeds-length-limit-rejected|run#7|AssertionError|assert msg == "Invalid LogGroup or LogStream, both must have length less than or equal to 512.", (
Issue-Key: logs.create-log-group-name-exceeds-max-length-rejected|run#7|AssertionError|assert msg == "Invalid LogGroup or LogStream, both must have length less than or equal to 512.", (
Issue-Key: logs.delete-integration-unknown-name-rejected|run#6|AssertionError|assert get_message == expected_message, (
Issue-Key: logs.get-integration-nonexistent-fails|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.delete-log-anomaly-detector-nonexistent-fails|run#14|AssertionError|assert gone_msg == _not_found(detector_arn), f"unexpected not-found message {gone_msg!r}"
Issue-Key: logs.get-deleted-log-anomaly-detector-fails|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.get-log-anomaly-detector-nonexistent-fails|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.delete-transformer-nonexistent-fails|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: logs.get-data-protection-policy-nonexistent-log-group-rejected|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.create-scheduled-query-duplicate-name-conflict-fails|run#13|AssertionError|assert msg == f"Scheduled query with identifier {query_name} already exists", (
Issue-Related: 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# logs: ten CloudWatch Logs refusal messages are emulator-authored instead of AWS's text for the error class (10 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAccountPolicy

## Description
Ten independent message-fidelity root causes in the logs service. Cause count: 10. Every cause below has its own failing check ids; all ten must be closed. In each one the HTTP status and error code are already correct — only the message diverges, and a message that differs from AWS's wording is itself the defect.

=== Cause 1 of 10: DeleteAccountPolicy on an absent account policy ===
AWS: ResourceNotFoundException (HTTP 400) whose message is AWS's fixed text for the class, with no variable parts: 'Specified resource does not exists!' (recorded verbatim, including the AWS typo, by account-policy-lifecycle, put-account-policy-then-describe and put-account-policy-concurrent-update-no-corruption; the variant without the trailing 's' — 'Specified resource does not exist!' — is what account-policy-lifecycle-transitions and delete-nonexistent-account-policy-fails recorded). Recorded behavior only for the text; Smithy binds ResourceNotFoundException to DeleteAccountPolicy.
Emulator: 'The specified account policy does not exist.' for every policy type (DATA_PROTECTION_POLICY, FIELD_INDEX_POLICY, SUBSCRIPTION_FILTER_POLICY, TRANSFORMER_POLICY).
Why: six checks' worth of clients that match the not-found text to make a delete idempotent see a sentence AWS never emits and fall through to a generic failure.
Fix: the not-found branch of the DeleteAccountPolicy handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_account_policy.rs) — use AWS's fixed text for the class.
Traces: logs.account-policy-lifecycle, logs.account-policy-lifecycle-transitions, logs.delete-account-policy-nonexistent-rejected, logs.delete-nonexistent-account-policy-fails, logs.put-account-policy-concurrent-update-no-corruption, logs.put-account-policy-then-describe.

=== Cause 2 of 10: PutAccountPolicy over the per-kind account quota ===
AWS: LimitExceededException with the fixed text 'Resource limit exceeded.' — no variable parts, no quota number.
Emulator: 'an account can have up to 1 account-level DATA_PROTECTION_POLICY policies of this kind' — an emulator-authored sentence that also leaks the kind and the limit.
Why: quota handling keyed on AWS's message does not fire; the message also implies a per-kind detail AWS does not expose here.
Fix: the quota branch of the PutAccountPolicy handler.
Traces: logs.account-policy-limit-exceeded-enforced.

=== Cause 3 of 10: PutAccountPolicy with a malformed policyDocument ===
AWS: InvalidParameterException with the fixed text 'Policy document contains invalid JSON format.'
Emulator: 'Fields in policyDocument must be an array of field names' — a field-level sentence of the emulator's own.
Why: a client cannot tell "the document is not valid JSON for this policy type" from a specific field complaint AWS never emits.
Fix: the policyDocument validation branch of the PutAccountPolicy handler.
Traces: logs.account-policy-malformed-document-rejected.

=== Cause 4 of 10: constraint-violation messages echo the ANCHORED model pattern ===
AWS: the standard constraint message is '1 validation error detected: Value at \'<member>\' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>'. Its fixed text is that template; its variable parts are the member name and the pattern, and AWS renders the pattern WITHOUT the ^...$ anchors: '[\\.\\-_/#A-Za-z0-9]+' for logGroupName, '[^:*]*' for logStreamName, and '(\\^?[\\.\\-_\\/#A-Za-z0-9]{3,512})|(\\^?[\\.\\-_\\/#A-Za-z0-9]{3,24})(\\|\\^?[\\.\\-_\\/#A-Za-z0-9]{3,24}){0,4}' for logGroupNamePattern. Smithy: LogGroupName @pattern '^[\\.\\-_/#A-Za-z0-9]+$', LogStreamName @pattern '^[^:*]*$'.
Emulator: it echoes the model's anchored pattern verbatim — '^[\\.\\-_/#A-Za-z0-9]+$', '^[^:*]*$', '^(\\^?[...]{3,24})(\\|\\^?[...]{3,24}){0,4}$' — and for logGroupNamePattern it also drops the {3,512} alternative.
Why: clients that assert on the validation message, and users who copy the pattern out of it to fix input, get a regex AWS never returns (and for logGroupNamePattern a strictly narrower one).
Fix: the request-validation message renderer shared by CreateLogGroup/CreateLogStream/DescribeLogGroups — strip the anchors (render the service's own pattern form) rather than the Smithy trait string.
Traces: logs.create-log-group-invalid-name-rejected, logs.create-log-stream-invalid-name-rejected, logs.invalid-parameter-validation-on-create, logs.list-validation-error-semantics.

=== Cause 5 of 10: an over-long logGroupName is refused with the generic constraint template ===
AWS: CreateLogGroup with a name longer than the modeled 512-character maximum (Smithy LogGroupName @length min 1 max 512) is refused with InvalidParameterException and the service's own fixed text 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.'
Emulator: '1 validation error detected: Value at \'logGroupName\' failed to satisfy constraint: Member must have length less than or equal to 512'.
Why: the length refusal is unrecognisable to a client matching AWS's sentence, even though the code is right.
Fix: the length-validation branch for logGroupName/logStreamName in the CreateLogGroup/CreateLogStream handlers — use the service's fixed text instead of the generic constraint renderer.
Traces: logs.create-log-group-name-exceeds-length-limit-rejected, logs.create-log-group-name-exceeds-max-length-rejected.

=== Cause 6 of 10: integration not-found messages do not name the integration ===
AWS: ResourceNotFoundException with 'Integration with name <integrationName> does not exist.' — fixed text around one variable part, the integrationName taken from the request.
Emulator: 'The specified integration does not exist.' — the identifier is dropped.
Why: a caller handling several integrations cannot tell which name was missing from the error alone.
Fix: the not-found branches of the GetIntegration and DeleteIntegration handlers.
Traces: logs.delete-integration-unknown-name-rejected, logs.get-integration-nonexistent-fails.

=== Cause 7 of 10: anomaly-detector not-found messages do not name the ARN ===
AWS: ResourceNotFoundException with 'The specified ARN does not exist or already deleted : <anomalyDetectorArn>' — fixed text (note the space before the colon) plus one variable part, the ARN from the request.
Emulator: 'The specified anomaly detector does not exist.'
Why: the ARN that failed is unrecoverable from the error, and AWS's deliberate "does not exist or already deleted" ambiguity is lost.
Fix: the not-found branches of GetLogAnomalyDetector and DeleteLogAnomalyDetector.
Traces: logs.delete-log-anomaly-detector-nonexistent-fails, logs.get-deleted-log-anomaly-detector-fails, logs.get-log-anomaly-detector-nonexistent-fails.

=== Cause 8 of 10: DeleteTransformer on a group with no transformer ===
AWS: ResourceNotFoundException with "Transformer does not exist on log group '<logGroupName>'" — fixed text plus the group name from the request.
Emulator: 'Specified resource does not exist!' — the generic account-policy sentence, with no group name.
Why: a client deleting transformers across groups cannot tell which group had none, and the message does not even name the resource kind.
Fix: the not-found branch of the DeleteTransformer handler.
Traces: logs.delete-transformer-nonexistent-fails.

=== Cause 9 of 10: GetDataProtectionPolicy on a missing log group ===
AWS: ResourceNotFoundException with "Log group '<logGroupName>' for owner '<accountId>' cannot be found" — fixed text plus two variable parts, the requested group name and the owning account id.
Emulator: 'The specified log group does not exist.'
Why: the message no longer identifies the group or the owner, which is exactly what makes AWS's text usable for cross-account diagnosis.
Fix: the not-found branch of the GetDataProtectionPolicy handler.
Traces: logs.get-data-protection-policy-nonexistent-log-group-rejected.

=== Cause 10 of 10: CreateScheduledQuery duplicate-name ConflictException ===
AWS: ConflictException with 'Scheduled query with identifier <name> already exists' — fixed text plus the `name` from the request.
Emulator: 'A scheduled query named <name> already exists'.
Why: the conflict is unrecognisable to tooling that matches AWS's "with identifier ... already exists" phrasing to choose between adopt-existing and fail.
Fix: the name-collision branch of the CreateScheduledQuery handler. This is the same root cause as the open issue 20261028T070223Z-logs-createscheduledquery-s-duplicate-name-conflictexception-message-diverges.md, which carries a different replay key (logs.conflict-exception-concurrent-state); the check below is not covered by it.
Traces: logs.create-scheduled-query-duplicate-name-conflict-fails.

## Evidence
All quotes below are from the behavioral replay; generated names/ARNs inside them are per-run values, never part of the requirement.
Cause 1: "expected the exact message 'Specified resource does not exists!', got 'The specified account policy does not exist.'"; "expected the exact message 'Specified resource does not exist!', got 'The specified account policy does not exist.'"; "unexpected message deleting an already-deleted FIELD_INDEX_POLICY: 'The specified account policy does not exist.'"; "expected message 'Specified resource does not exist!', got 'The specified account policy does not exist.'"; two further checks assert `msg == "Specified resource does not exists!"` and report "got 'The specified account policy does not exist.'".
Cause 2: "expected the exact message 'Resource limit exceeded.', got 'an account can have up to 1 account-level DATA_PROTECTION_POLICY policies of this kind'".
Cause 3: "expected the exact message 'Policy document contains invalid JSON format.', got 'Fields in policyDocument must be an array of field names'".
Cause 4: "unexpected message: \"1 validation error detected: Value at 'logGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\.\\-_/#A-Za-z0-9]+$\""; "... Value at 'logStreamName' ... pattern: ^[^:*]*$"; "expected message \"... pattern: [\\.\\-_/#A-Za-z0-9]+\", got \"... pattern: ^[\\.\\-_/#A-Za-z0-9]+$\""; "expected message \"... Value at 'logGroupNamePattern' ... pattern: (\\^?[\\.\\-_\\/#A-Za-z0-9]{3,512})|(\\^?[...]{3,24})(\\|\\^?[...]{3,24}){0,4}\", got \"... pattern: ^(\\^?[...]{3,24})(\\|\\^?[...]{3,24}){0,4}$\"".
Cause 5: both checks assert `msg == "Invalid LogGroup or LogStream, both must have length less than or equal to 512."` and report "unexpected message: \"1 validation error detected: Value at 'logGroupName' failed to satisfy constraint: Member must have length less than or equal to 512\"".
Cause 6: "expected message 'Integration with name <name> does not exist.', got 'The specified integration does not exist.'" (twice).
Cause 7: "expected message 'The specified ARN does not exist or already deleted : <arn>', got 'The specified anomaly detector does not exist.'" (twice) and "unexpected not-found message 'The specified anomaly detector does not exist.'".
Cause 8: "expected message \"Transformer does not exist on log group '<group>'\", got 'Specified resource does not exist!'".
Cause 9: "expected message \"Log group '<group>' for owner '<account>' cannot be found\", got 'The specified log group does not exist.'".
Cause 10: check asserts `msg == f"Scheduled query with identifier {query_name} already exists"`; replay: "unexpected duplicate-name CreateScheduledQuery message: 'A scheduled query named <name> already exists'".

## Steps to Reproduce
Cause 1: logs PutAccountPolicy(policyType=DATA_PROTECTION_POLICY) then DeleteAccountPolicy twice (or DeleteAccountPolicy for a never-created FIELD_INDEX_POLICY). Compare the second call's ResourceNotFoundException message with AWS's fixed text.
Cause 2: PutAccountPolicy twice for the same policyType with different names until the per-kind quota is hit; compare the LimitExceededException message with 'Resource limit exceeded.'
Cause 3: PutAccountPolicy with a policyDocument whose Statement[].Operation.Audit.FindingsDestination is well-formed but whose Fields member is not an array; compare the InvalidParameterException message with 'Policy document contains invalid JSON format.'
Cause 4: CreateLogGroup with a name containing an illegal character; CreateLogStream with a name containing ':'; DescribeLogGroups with an invalid logGroupNamePattern. Compare the echoed regex with AWS's unanchored form.
Cause 5: CreateLogGroup with a 513-character name. Expect InvalidParameterException 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.'
Cause 6: GetIntegration / DeleteIntegration with an integrationName that does not exist.
Cause 7: GetLogAnomalyDetector / DeleteLogAnomalyDetector with a well-formed but unknown anomaly-detector ARN.
Cause 8: CreateLogGroup then DeleteTransformer on it without ever calling PutTransformer.
Cause 9: GetDataProtectionPolicy logGroupIdentifier=<a log group that does not exist>.
Cause 10: CreateScheduledQuery twice with the same name; compare the ConflictException message.
