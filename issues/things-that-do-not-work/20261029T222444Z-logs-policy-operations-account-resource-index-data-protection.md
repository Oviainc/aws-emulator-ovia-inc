Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-policy-suite-with-data-protection
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-policy-suite-with-data-protection logs-log-group-kms-and-data-protection logs-resource-and-account-policy-management logspc-index-policy-put-describe-delete logs-log-group-configuration-lifecycle logspc-kms-key-associate-disassociate
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|GetDataProtectionPolicy|2xx->4xx|$
Issue-Key: logs|GetDataProtectionPolicy|2xx->4xx|status
Issue-Key: logs|PutAccountPolicy|4xx->4xx|$.message
Issue-Key: logs|DeleteAccountPolicy|4xx->4xx|$.message
Issue-Key: logs|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: logs|PutIndexPolicy|4xx->4xx|$.message
Issue-Key: logs|DeleteDataProtectionPolicy|4xx->4xx|$.message
Issue-Key: logs|PutDataProtectionPolicy|4xx->4xx|$.message
Issue-Key: logs|AssociateKmsKey|4xx->2xx|$
Issue-Key: logs|AssociateKmsKey|4xx->2xx|status
Issue-Key: logs|GetDataProtectionPolicy|4xx->4xx|$.message
Issue-Key: logs|DescribeIndexPolicies|4xx->4xx|$.message
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs policy operations (account, resource, index, data-protection, KMS) diverge on status and message (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataProtectionPolicy

## Description
Ten root causes across the CloudWatch Logs policy surfaces (account policies, resource policies, index policies, data-protection policies) and the KMS association path. One read returns the wrong STATUS, one write accepts a key AWS refuses, and eight refusal messages are emulator-authored instead of AWS's text for the error class. Cause count: 10 — all ten must be closed. The open behavioral issue 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md covers the account-policy and data-protection wording from behavioral checks and carries different replay keys; the wire steps below are not covered by it.

=== Cause 1 of 10: GetDataProtectionPolicy on an existing log group with no policy answers 400 ===
AWS: HTTP 200 with an empty body for a log group that exists but has no data-protection policy.
Emulator: HTTP 400 ResourceNotFoundException 'Specified resource does not exist!'.
Why: a client probing whether masking is configured gets a hard error and cannot distinguish 'no policy' from 'no log group'; put-if-absent logic aborts.
Fix: the no-policy branch of the GetDataProtectionPolicy handler — only the missing-log-group case is a ResourceNotFoundException (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-b2-policy-suite-with-data-protection#task#12 (also logs-log-group-kms-and-data-protection#task#5).

=== Cause 2 of 10: AssociateKmsKey accepts a kmsKeyId that does not resolve ===
AWS: refuses with HTTP 400 InvalidParameterException and the fixed text 'The specified KMS Key Id could not be found.' (no variable parts). Smithy: AssociateKmsKey binds InvalidParameterException (httpError 400).
Emulator: answers HTTP 200 with an empty body and records the association.
Why: the log group appears CMK-encrypted with a key that does not exist; a client that treats the success as proof of encryption is wrong, and the later disassociate/rotate flow diverges.
Fix: the kmsKeyId resolution check in the AssociateKmsKey handler.
Traces: logs-log-group-configuration-lifecycle#task#9 (also logs-log-group-kms-and-data-protection#task#7, logspc-kms-key-associate-disassociate#task#3 and #task#4).

=== Cause 3 of 10: PutAccountPolicy's malformed-JSON refusal is emulator-authored ===
AWS: when policyDocument is not parseable JSON the message is the service's Jackson parse error, fixed in form: "Invalid policy JSON: Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]" — the fixed text is that template; the variable parts are the offending token and the line/column taken from the request body.
Emulator: 'policyDocument must be a data protection policy in JSON (a JSON object)'.
Why: a caller cannot locate the syntax error, and assertions on AWS's parse-error text fail even though status 400 is right.
Fix: the policyDocument parse branch of the PutAccountPolicy handler.
Traces: logs-b2-policy-suite-with-data-protection#task#6.

=== Cause 4 of 10: DeleteAccountPolicy's not-found refusal is emulator-authored ===
AWS: ResourceNotFoundException (HTTP 400) with the fixed text 'Specified resource does not exists!' (AWS's own typo, no variable parts).
Emulator: 'The specified account policy does not exist.'
Why: delete-idempotence logic keyed on AWS's sentence falls through to a generic failure.
Fix: the not-found branch of the DeleteAccountPolicy handler.
Traces: logs-b2-policy-suite-with-data-protection#task#7 (also logs-resource-and-account-policy-management#teardown#11).

=== Cause 5 of 10: PutResourcePolicy's invalid-policy refusal is emulator-authored ===
AWS: refuses with the fixed text 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.' (no variable parts).
Emulator: 'policyDocument must be a JSON policy document (a JSON object)' — and it names a member (policyDocument) rather than accessPolicy.
Why: a client cannot tell that the document failed IAM-grammar parsing, and the member named in the message does not exist on this request.
Fix: the policy-parse branch of the PutResourcePolicy handler.
Traces: logs-b2-policy-suite-with-data-protection#task#8 (also logs-resource-and-account-policy-management#task#3).

=== Cause 6 of 10: PutIndexPolicy's malformed-document refusal is emulator-authored ===
AWS: refuses with the fixed text 'Policy document contains invalid JSON format.' (no variable parts).
Emulator: 'policyDocument must be the policy in JSON (a non-empty JSON object)'.
Why: same message-fidelity failure on the field-index policy surface.
Fix: the policyDocument validation branch of the PutIndexPolicy handler.
Traces: logs-b2-policy-suite-with-data-protection#task#10 (also logspc-index-policy-put-describe-delete#task#5).

=== Cause 7 of 10: DeleteDataProtectionPolicy's not-found refusal is emulator-authored ===
AWS: 'In DeleteDataProtectionPolicyInternalImpl request, there is no related DataProtectionPolicy with policyKey <accountId>:<logGroupName>.' — fixed text around one variable part, the policy key built from the caller's account id and the requested log group.
Emulator: 'Specified resource does not exist!' — the generic account-policy sentence, with no policy key.
Why: the message no longer identifies which group had no policy, which is the only diagnostic value it has.
Fix: the not-found branch of the DeleteDataProtectionPolicy handler.
Traces: logs-b2-policy-suite-with-data-protection#task#13.

=== Cause 8 of 10: PutDataProtectionPolicy's malformed-JSON refusal is emulator-authored ===
AWS: the same Jackson parse-error template as cause 3: "Invalid policy JSON: Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]".
Emulator: 'policyDocument must be a data protection policy in JSON (a JSON object)'.
Why: same as cause 3 on the per-log-group data-protection path.
Fix: the policyDocument parse branch of the PutDataProtectionPolicy handler (share the renderer with PutAccountPolicy).
Traces: logs-b2-policy-suite-with-data-protection#task#14.

=== Cause 9 of 10: GetDataProtectionPolicy's missing-log-group message drops the group and owner ===
AWS: ResourceNotFoundException with "Log group '<logGroupName>' for owner '<accountId>' cannot be found" — fixed text plus two variable parts from the request and the caller identity.
Emulator: 'The specified log group does not exist.'
Why: cross-account diagnosis is impossible without the group name and owner that AWS puts in the text.
Fix: the missing-log-group branch of the GetDataProtectionPolicy handler.
Traces: logs-log-group-kms-and-data-protection#task#6.

=== Cause 10 of 10: DescribeIndexPolicies' missing-log-group message is emulator-authored ===
AWS: refuses with the fixed text 'Log group cannot be found.' (no variable parts).
Emulator: 'The specified log group does not exist.'
Why: status and code are right but the sentence differs, so message-matching clients mis-handle it.
Fix: the missing-log-group branch of the DescribeIndexPolicies handler.
Traces: logspc-index-policy-put-describe-delete#task#9.

## Evidence
Cause 1: "status: expected 200, got 400; $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'Specified resource does not exist!'})" — 2 affected steps.
Cause 2: "status: expected 400, got 200; $: empty_body (expected {'__type': 'InvalidParameterException', 'message': 'The specified KMS Key Id could not be found.'}, got {})" — 4 affected steps.
Cause 3: "$.message: value_mismatch (expected \"Invalid policy JSON: Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]\", got 'policyDocument must be a data protection policy in JSON (a JSON object)')" — 'this' and line/column are the request-derived variable parts.
Cause 4: "$.message: value_mismatch (expected 'Specified resource does not exists!', got 'The specified account policy does not exist.')" — 2 affected steps.
Cause 5: "$.message: value_mismatch (expected 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.', got 'policyDocument must be a JSON policy document (a JSON object)')" — 2 affected steps.
Cause 6: "$.message: value_mismatch (expected 'Policy document contains invalid JSON format.', got 'policyDocument must be the policy in JSON (a non-empty JSON object)')" — 2 affected steps.
Cause 7: "$.message: value_mismatch (expected 'In DeleteDataProtectionPolicyInternalImpl request, there is no related DataProtectionPolicy with policyKey 000000000000:<logGroupName>.', got 'Specified resource does not exist!')" — account id and group name are the variable parts.
Cause 8: "$.message: value_mismatch (expected \"Invalid policy JSON: Unrecognized token 'not': ... line: 1, column: 1]\", got 'policyDocument must be a data protection policy in JSON (a JSON object)')".
Cause 9: "$.message: value_mismatch (expected \"Log group '<logGroupName>' for owner '000000000000' cannot be found\", got 'The specified log group does not exist.')".
Cause 10: "$.message: value_mismatch (expected 'Log group cannot be found.', got 'The specified log group does not exist.')".

## Steps to Reproduce
1: CreateLogGroup, then GetDataProtectionPolicy on it without PutDataProtectionPolicy — expect HTTP 200 with an empty body (logs-b2-policy-suite-with-data-protection#task#12).
2: CreateLogGroup, then AssociateKmsKey(logGroupName=<it>, kmsKeyId=<a well-formed key ARN that does not exist>) — expect 400 InvalidParameterException 'The specified KMS Key Id could not be found.' (logs-log-group-configuration-lifecycle#task#9).
3: PutAccountPolicy(policyDocument='this is not json') — compare with AWS's 'Invalid policy JSON: Unrecognized token ...' text (#task#6).
4: DeleteAccountPolicy for a policy type never put — expect 'Specified resource does not exists!' (#task#7).
5: PutResourcePolicy(policyDocument=<not an IAM policy>) — expect the accessPolicy IAM-grammar sentence (#task#8).
6: PutIndexPolicy(policyDocument=<malformed>) — expect 'Policy document contains invalid JSON format.' (#task#10).
7: DeleteDataProtectionPolicy on a group with no policy — expect the policyKey sentence (#task#13).
8: PutDataProtectionPolicy(policyDocument='not json') — expect the parse-error template (#task#14).
9: GetDataProtectionPolicy(logGroupIdentifier=<a group that does not exist>) — expect "Log group '<name>' for owner '<acct>' cannot be found" (logs-log-group-kms-and-data-protection#task#6).
10: DescribeIndexPolicies(logGroupIdentifiers=[<a group that does not exist>]) — expect 'Log group cannot be found.' (logspc-index-policy-put-describe-delete#task#9).
