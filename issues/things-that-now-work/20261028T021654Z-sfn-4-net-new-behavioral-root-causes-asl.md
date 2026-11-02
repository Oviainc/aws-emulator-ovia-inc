Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected stepfunctions.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected stepfunctions.deep-sfn-2-sfn-jsonata-pass-states-result-rejected stepfunctions.error-kms-access-denied-describe-execution
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected|run#12|AssertionError|assert messages == [
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected|run#10|AssertionError|assert v["diagnostics"][0]["message"] == (
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-pass-states-result-rejected|run#17|AssertionError|assert message == (
Issue-Key: stepfunctions.error-kms-access-denied-describe-execution|run#38|AssertionError|assert _stable(message) == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: a7f4b46fd13e3042ad499c1371bfc738191ffa75

# sfn: 4 net-new behavioral root causes — ASL validation diagnostics name only the reserved term, the JSONata/JSONPath clash message is emulator-authored, InvalidDefinition parenthesizes the location, and the relayed KMS refusal trailer uses the old SDK format

> _Note: this issue was identified against the code base at commit `a7f4b46fd13e3042ad499c1371bfc738191ffa75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateStateMachineDefinition / CreateStateMachine / DescribeExecution

## Description
Four net-new root causes in the Step Functions emulator. They are bundled because they all belong to one service, but they are FOUR independent causes and each must be closed on its own: causes 1-3 live in the ASL validation/diagnostic-reporting walk reached by ValidateStateMachineDefinition and CreateStateMachine, cause 4 lives in the KMS-refusal relay on the execution read path. In every one of the four the classification (result code, diagnostic code, location, error code) is already correct and only the message the caller is handed diverges; no cause here is about the interpreter failing to run a workflow.

Related open issues that do NOT cover these: 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md (control-plane authorization and the oversized-input message, not these diagnostic texts); 20261027T000406Z-sfn-step-functions-states-startexecution-sync-is-never-executed.md (carries sfn.describe-execution-kms-access-denied — the CMK-encrypted execution that never advances — not the relayed KmsAccessDeniedException message of cause 4); 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md (execution of the language, not its validation). None of them carries any of the four replay keys below.

Cause 1 of 4: a diagnostic for a reserved term that the expression's scope does not bind names only the reserved prefix, not the field the expression actually read.
AWS: Smithy: ValidateStateMachineDefinition returns `diagnostics` of ValidateStateMachineDefinitionDiagnostic {severity, code, message, location}, location documented as `/States/<StateName>/<FieldName>`; `$states.errorOutput` is bound only inside a Catch. Exact message text is recorded behavior only. For a JSONata Task whose OWN `Output` reads `{"error": "{% $states.errorOutput.Error %}", "cause": "{% $states.errorOutput.Cause %}"}`, AWS answers result=FAIL with one ERROR/UNSUPPORTED_JSONATA_EXPRESSION diagnostic per field: message "Field '$states.errorOutput.Error' does not exist." at `/States/<StateName>/Output/error` and "Field '$states.errorOutput.Cause' does not exist." at `/States/<StateName>/Output/cause`. The fixed text is "Field '" … "' does not exist."; the variable part is the FULL field expression taken from the submitted definition (reserved term plus the member accessors written after it).
Emulator: severity, code and location are right, but BOTH messages are "Field '$states.errorOutput' does not exist." — truncated to the reserved term, so the two diagnostics are textually identical and neither names the member that could not be bound. (A bare `$states.result` happens to match only because there the written expression equals the term.)
Why: a CI gate or console that shows the diagnostic message points at the reserved prefix instead of the expression to fix, and two distinct problems read as one repeated message. The same string is embedded verbatim in the CreateStateMachine InvalidDefinition message, so a deployment's refusal text is wrong for the same reason.
Fix: the out-of-scope reserved-term check in the sfn crate's ASL diagnostic walk — report the field expression that was read, not the bare term (see Traces for the exact formatter).
Traces: sfn.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected. The message is produced by `field_does_not_exist(term)` at services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs:1707-1709, called with the reserved term (not the read field) at src/state/asl.rs:1777-1782.

Cause 2 of 4: the SCHEMA_VALIDATION_FAILED message for a per-state `QueryLanguage: JSONPath` override inside a JSONata state machine is emulator-authored.
AWS: recorded behavior only (the Smithy model documents the diagnostic structure and the SCHEMA_VALIDATION_FAILED code, not its text). A machine with top-level `"QueryLanguage": "JSONata"` containing a state with `"QueryLanguage": "JSONPath"` validates FAIL with exactly one ERROR diagnostic, code SCHEMA_VALIDATION_FAILED, location `/States/<StateName>`, message "'QueryLanguage' can not be 'JSONPath' if set to 'JSONata' for whole state machine" — entirely fixed text; the state is identified by `location`, not by the sentence. CreateStateMachine then refuses InvalidDefinition with "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: 'QueryLanguage' can not be 'JSONPath' if set to 'JSONata' for whole state machine at /States/<StateName>'".
Emulator: same severity/code/location, but the message is the emulator's own sentence "state '<StateName>' selects 'JSONPath', which a state of a 'JSONata' state machine must not do", which also interpolates the state name into text AWS keeps fixed.
Why: the refusal a client sees for the most common JSONata migration mistake is wording no AWS client, doc or runbook can match, and the emulator-voiced phrasing ("must not do") identifies the emulator rather than an AWS error condition; tooling that keys on the AWS sentence sees an unknown message.
Fix: the per-state QueryLanguage clash diagnostic in the sfn crate's ASL validation walk — emit AWS's fixed sentence and leave the state name to `location`.
Traces: sfn.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected. Emitted at services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs:1576-1583.

Cause 3 of 4: the InvalidDefinition message parenthesizes the diagnostic's location.
AWS: Smithy: CreateStateMachine/UpdateStateMachine model InvalidDefinition (HTTP 400, "The provided Amazon States Language definition is not valid."). Recorded message shape: "Invalid State Machine Definition: '" + one subject per diagnostic, comma-and-space separated, + "'", where a subject is "<CODE>: <message> at <location>" — the connective is the bare word ` at `, and `<CODE>`, `<message>` and `<location>` are the values of the diagnostics computed for the submitted definition.
Emulator: the subject is rendered "<CODE>: <message> (at <location>)" — the location is wrapped in parentheses, e.g. "Invalid State Machine Definition: 'UNSUPPORTED_JSONATA_EXPRESSION: Field '$states.result' does not exist. (at /States/P/Output/x)'" where AWS returns "… does not exist. at /States/P/Output/x'". Code, diagnostic text and location are otherwise right.
Why: every InvalidDefinition message the emulator produces differs from AWS's, so any client, test or deployment gate that matches the refusal text (or parses the location out of it) fails on a definition AWS refuses with the same classification. It affects all rejected definitions, not only the JSONata ones.
Fix: the InvalidDefinition subject formatter in the sfn crate — drop the parentheses so the location is joined with ` at `.
Traces: sfn.deep-sfn-2-sfn-jsonata-pass-states-result-rejected. `Diagnostic::stated` at services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs:1135-1142, and the sibling formatter at src/state/asl.rs:2153-2161.

Cause 4 of 4: the KMS refusal that DescribeExecution relays carries an old-SDK trailer instead of the one AWS appends.
AWS: Smithy: DescribeExecution models KmsAccessDeniedException (HTTP 400, "Either your KMS key policy or API caller does not have the required permissions."). Recorded message shape: the inner KMS not-authorized sentence ("User: <caller session arn> is not authorized to perform: kms:Decrypt on resource: <key arn> because no identity-based policy allows the kms:Decrypt action") followed by the relayed SDK trailer " (Service: Kms, Status Code: 400, Request ID: <inner KMS request id>) (SDK Attempt Count: <n>)". Fixed text: the field names `Service:`/`Status Code:`/`Request ID:` separated by ", " inside ONE parenthesised group with service token `Kms`, and a SECOND group "(SDK Attempt Count: <n>)"; the only per-call values are the inner request id and the attempt count. No `Error Code:` and no `Proxy:` field appear.
Emulator: the error code is correctly KmsAccessDeniedException and the not-authorized sentence matches exactly, but the trailer is " (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)" — semicolon-separated, service token `AWSKMS`, two extra fields (`Error Code`, `Proxy: null`), and the `(SDK Attempt Count: <n>)` group is missing entirely.
Why: the relayed trailer is the only way a caller correlates the Step Functions refusal with the inner KMS call (its request id) and sees how many attempts were made; a client or log pipeline that parses the AWS trailer extracts nothing, and the `Proxy: null`/`AWSKMS` tokens belong to an SDK generation AWS no longer emits here.
Fix: the KMS-refusal relay on the sfn execution read path — the component that composes KmsAccessDeniedException.message from the inner KMS error when DescribeExecution decrypts an execution of a CMK-encrypted state machine; render AWS's comma-separated `(Service: Kms, Status Code: …, Request ID: …)` group plus the `(SDK Attempt Count: …)` group.
Traces: sfn.error-kms-access-denied-describe-execution.

## Evidence
Cause 1 — sfn.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected (ValidateStateMachineDefinition on a JSONata machine whose Task Output reads $states.errorOutput.Error/.Cause):
  expected diagnostics (location, message):
    ('/States/T/Output/cause', "Field '$states.errorOutput.Cause' does not exist.")
    ('/States/T/Output/error', "Field '$states.errorOutput.Error' does not exist.")
  actual:
    AssertionError: diagnostic messages [('/States/T/Output/cause', "Field '$states.errorOutput' does not exist."), ('/States/T/Output/error', "Field '$states.errorOutput' does not exist.")]
  (result=FAIL, severity=ERROR, code=UNSUPPORTED_JSONATA_EXPRESSION and both locations already matched; only the message text diverges. The same text is then embedded in CreateStateMachine's InvalidDefinition message, which AWS words "Invalid State Machine Definition: 'UNSUPPORTED_JSONATA_EXPRESSION: Field '$states.errorOutput.Error' does not exist. at /States/T/Output/error, UNSUPPORTED_JSONATA_EXPRESSION: Field '$states.errorOutput.Cause' does not exist. at /States/T/Output/cause'".)

Cause 2 — sfn.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected (definition: {"QueryLanguage":"JSONata","StartAt":"P","States":{"P":{"Type":"Pass","QueryLanguage":"JSONPath","Next":"Q"}, "Q":{...}}}):
  expected message: "'QueryLanguage' can not be 'JSONPath' if set to 'JSONata' for whole state machine"
  actual:   AssertionError: unexpected diagnostic message "state 'P' selects 'JSONPath', which a state of a 'JSONata' state machine must not do"
  (result=FAIL and the single ('ERROR','SCHEMA_VALIDATION_FAILED','/States/P') diagnostic already matched. 'P' is the state name from the request; in AWS's wording the state appears only in `location`.)

Cause 3 — sfn.deep-sfn-2-sfn-jsonata-pass-states-result-rejected (CreateStateMachine with a JSONata Pass whose Output reads $states.result):
  expected message: "Invalid State Machine Definition: 'UNSUPPORTED_JSONATA_EXPRESSION: Field '$states.result' does not exist. at /States/P/Output/x'"
  actual:   AssertionError: unexpected InvalidDefinition message "Invalid State Machine Definition: 'UNSUPPORTED_JSONATA_EXPRESSION: Field '$states.result' does not exist. (at /States/P/Output/x)'"
  (error code InvalidDefinition and the ValidateStateMachineDefinition diagnostic for the same definition both matched; `/States/P/Output/x` is the location computed from the submitted definition, the divergence is the parentheses around "at <location>".)

Cause 4 — sfn.error-kms-access-denied-describe-execution (DescribeExecution by a caller whose kms:Decrypt grant on the state machine's CMK was revoked):
  error code: KmsAccessDeniedException (matches AWS)
  expected message: 'User: arn:aws:sts::<account>:assumed-role/<caller role>/<session> is not authorized to perform: kms:Decrypt on resource: <key arn> because no identity-based policy allows the kms:Decrypt action (Service: Kms, Status Code: 400, Request ID: <request-id>) (SDK Attempt Count: <n>)'
  actual message:   'User: arn:aws:sts::<account>:assumed-role/<caller role>/<session> is not authorized to perform: kms:Decrypt on resource: <key arn> because no identity-based policy allows the kms:Decrypt action (Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: <request-id>; Proxy: null)'
  (the check normalises the per-call request id and attempt count before comparing, so the divergence is purely the trailer's fixed text: comma vs semicolon separators, `Kms` vs `AWSKMS`, the extra `Error Code`/`Proxy` fields, and the missing `(SDK Attempt Count: <n>)` group.)

## Steps to Reproduce
Cause 1:
1. ValidateStateMachineDefinition with type=STANDARD and definition {"QueryLanguage":"JSONata","StartAt":"T","States":{"T":{"Type":"Task","Resource":"arn:aws:states:::aws-sdk:sts:getCallerIdentity","Arguments":{},"Output":{"error":"{% $states.errorOutput.Error %}","cause":"{% $states.errorOutput.Cause %}"},"Next":"Done"},"Done":{"Type":"Pass","End":true}}}.
2. Read the two diagnostics' `message` fields: both read "Field '$states.errorOutput' does not exist." instead of naming `.Error` / `.Cause`.
3. CreateStateMachine with the same definition and read the InvalidDefinition message: it embeds the same truncated text.

Cause 2:
1. ValidateStateMachineDefinition with definition {"QueryLanguage":"JSONata","StartAt":"P","States":{"P":{"Type":"Pass","QueryLanguage":"JSONPath","Next":"Q"},"Q":{"Type":"Pass","Output":{"v":"{% $states.input.v %}"},"End":true}}}.
2. The single SCHEMA_VALIDATION_FAILED diagnostic at /States/P carries the emulator's "state 'P' selects 'JSONPath', …" sentence instead of "'QueryLanguage' can not be 'JSONPath' if set to 'JSONata' for whole state machine".
3. CreateStateMachine with the same definition: the InvalidDefinition message carries the same emulator sentence.

Cause 3:
1. CreateStateMachine (STANDARD) with definition {"QueryLanguage":"JSONata","StartAt":"Seed","States":{"Seed":{"Type":"Pass","Output":{"n":"{% $states.input.n + 1 %}"},"Next":"P"},"P":{"Type":"Pass","Output":{"x":"{% $states.result %}"},"End":true}}}.
2. The call is correctly refused InvalidDefinition, but the message renders the location as "(at /States/P/Output/x)" instead of " at /States/P/Output/x". Any definition with two or more error diagnostics shows the same wrapping on each subject.

Cause 4:
1. KMS CreateKey (symmetric, ENCRYPT_DECRYPT) with a key policy granting the account root, the harness principal and a freshly created caller role kms:Decrypt/kms:DescribeKey; grant the state machine's execution role kms:GenerateDataKey/kms:Decrypt/kms:DescribeKey.
2. CreateStateMachine with encryptionConfiguration {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key arn>} and StartExecution.
3. Assume the caller role (it holds states:DescribeExecution only) and confirm DescribeExecution on that execution succeeds.
4. PutKeyPolicy removing only the caller role's statement, then call DescribeExecution again with the caller role's credentials.
5. The refusal is KmsAccessDeniedException as expected, but its message ends "(Service: AWSKMS; Status Code: 400; Error Code: AccessDeniedException; Request ID: …; Proxy: null)" instead of "(Service: Kms, Status Code: 400, Request ID: …) (SDK Attempt Count: …)".
