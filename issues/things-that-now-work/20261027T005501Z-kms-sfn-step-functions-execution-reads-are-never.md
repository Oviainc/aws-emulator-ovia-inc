Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__stepfunctions.xsvc-sfn-3-disabled-cmk-execution-reads-rejected-until-reenabled
Issue-Key: kms__stepfunctions.xsvc-sfn-3-disabled-cmk-execution-reads-rejected-until-reenabled|run#18|Timeout|time.sleep(3.0)
Issue-Commit: d558c4fc1331790598d7d8dd6ea4bd673681392c

# kms__sfn: Step Functions execution reads are never refused after the state machine's CMK is disabled, so the disabled-CMK/re-enable contract hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `d558c4fc1331790598d7d8dd6ea4bd673681392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExecution

## Description
Cause 1 of 1 — a CMK-encrypted state machine's execution data stays readable after its KMS key is disabled: DescribeExecution and GetExecutionHistory keep answering 200 where AWS refuses them, so the check's refusal poll never breaks and the check is cut off at the harness cap.

AWS: a state machine created with encryptionConfiguration {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <CMK ARN>, kmsDataKeyReusePeriodSeconds: 60} holds its execution input/output and history encrypted under that CMK. Step Functions may serve reads from a cached data key for at most kmsDataKeyReusePeriodSeconds; once that window has run out, with the CMK in state Disabled, both DescribeExecution and GetExecutionHistory are refused with KmsInvalidStateException. Smithy: KmsInvalidStateException is modeled on BOTH DescribeExecution and GetExecutionHistory (errorKind client, HTTP 400), alongside KmsAccessDeniedException and KmsThrottlingException — the KMS-state-dependent refusal of a read is part of the modeled contract, not an edge case. The message is the text Step Functions relays from KMS: fixed text "<key ARN> is disabled. (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)", whose variable parts are the key ARN (from the state machine's encryptionConfiguration) and the relayed KMS request id (a per-call token, not part of the template). The refusal must REVERSE: after EnableKey, both reads must succeed again and return the same input, the same output, and the same history event sequence as before the key was touched.

Emulator: the check does not reach either assertion — `Error: Check timed out after 60.0s`. Its refusal loop polls DescribeExecution and GetExecutionHistory every 3 s with a deadline of kmsDataKeyReusePeriodSeconds + 90 s = 150 s and breaks only when BOTH calls refuse; while they keep succeeding it cannot break, so the 150 s loop runs past the 60 s harness cap. The positive legs before it (CreateKey, CreateRole/PutRolePolicy, CreateStateMachine with encryptionConfiguration, StartExecution, the execution reaching SUCCEEDED, DescribeExecution reading the input back, GetExecutionHistory, DisableKey, and DescribeKey confirming KeyState == 'Disabled') are the setup it had to clear to get there. Hypothesis: the emulator stores encryptionConfiguration as an inert flag — execution data is never actually wrapped under the CMK and no read consults the key's state — which is why the refusal never appears; the alternative, that one of the setup legs itself hangs, would be the same work item in the same handler path.

Why: a client cannot test or demonstrate the CMK kill-switch that CMK-encrypted state machines exist to provide. Disabling the key is the documented way to make execution data unreadable (a break-glass / incident-response control, and the basis of a crypto-shredding posture); against the emulator the data keeps flowing in plaintext, so an operator validating that control gets a false pass and discovers in production that the refusal path — and its reversal — behave differently. The absent KmsInvalidStateException also means no client error handler for this modeled fault is ever exercised.

Fix: bind Step Functions' execution-data read path to KMS key state — DescribeExecution and GetExecutionHistory in services/service-com-amazonaws-sfn-awsstepfunctions must resolve the state machine's encryptionConfiguration CMK and refuse with KmsInvalidStateException (HTTP 400) while that key is Disabled, honouring kmsDataKeyReusePeriodSeconds as the window during which a cached data key may still serve, and must track the key back to usable (not latch the failure) after EnableKey. The relayed message belongs in that refusal's serializer.

Traces: kms__sfn.xsvc-sfn-3-disabled-cmk-execution-reads-rejected-until-reenabled.

## Evidence
kms__sfn.xsvc-sfn-3-disabled-cmk-execution-reads-rejected-until-reenabled [mismatch]:
  Error: Check timed out after 60.0s

Why the timeout is the divergence: the check's refusal loop is
    deadline = time.monotonic() + _REUSE_S + 90.0        # _REUSE_S = 60 -> 150 s
    while time.monotonic() < deadline:
        describe_code, describe_msg = _error(lambda: sfn.describe_execution(executionArn=arn))
        history_code, history_msg  = _error(lambda: sfn.get_execution_history(executionArn=arn))
        if describe_code != "<call succeeded>" and history_code != "<call succeeded>":
            break
        time.sleep(3.0)
It breaks only once BOTH reads refuse. While both keep returning 200 it runs the full 150 s and is cut off by the 60 s harness cap, so neither of the two assertions below is ever evaluated.

Expected (AWS), after DisableKey and once the kmsDataKeyReusePeriodSeconds window has elapsed:
  DescribeExecution(executionArn)  -> KmsInvalidStateException, HTTP 400,
    Message "<CMK ARN> is disabled. (Service: Kms, Status Code: 400, Request ID: <KMS request id>) (SDK Attempt Count: 1)"
  GetExecutionHistory(executionArn) -> the same code and the same relayed message
  then after EnableKey: both reads succeed again, describe_execution['input'] == the original payload,
    ['output'] == the pre-disable output, and the GetExecutionHistory event-type sequence is identical to before.
Smithy: KmsInvalidStateException is modeled on DescribeExecution and GetExecutionHistory (client, httpError 400 defaulted), with KmsAccessDeniedException and KmsThrottlingException.

Actual (emulator): no refusal is ever observed within the window; the check is cut off at 60 s with no assertion reached. State established before the loop: DescribeKey reports KeyState == 'Disabled' (the DisableKey leg and its assertion are inside the check ahead of the loop), the execution had already reached status SUCCEEDED, and DescribeExecution had already read the input back verbatim as the positive control.
Corroborating: 2 of 3 kms__sfn checks in this suite fail, and the suite's other CMK-state-dependent reads across services show the same over-permissive shape.

## Steps to Reproduce
1. Replay the behavioral suite and re-run check kms__sfn.xsvc-sfn-3-disabled-cmk-execution-reads-rejected-until-reenabled.
2. Or by hand against the emulator: kms.create_key(KeyUsage='ENCRYPT_DECRYPT') -> <CMK ARN>.
3. iam.create_role with a states.amazonaws.com trust policy and iam.put_role_policy granting kms:Decrypt and kms:GenerateDataKey on <CMK ARN>.
4. stepfunctions.create_state_machine(name=..., definition=<a single Pass state that reshapes its input>, roleArn=<role>, type='STANDARD', encryptionConfiguration={'type': 'CUSTOMER_MANAGED_KMS_KEY', 'kmsKeyId': <CMK ARN>, 'kmsDataKeyReusePeriodSeconds': 60}).
5. stepfunctions.start_execution(stateMachineArn=..., input='{"marker": "...", "items": [1, "two", {"three": 3.5}]}'); poll describe_execution until status == 'SUCCEEDED'; record its input, output and the get_execution_history event-type sequence.
6. kms.disable_key(KeyId=<CMK>); confirm kms.describe_key reports KeyState == 'Disabled'.
7. Poll stepfunctions.describe_execution(executionArn) and stepfunctions.get_execution_history(executionArn) every 3 s for up to 150 s — observe both keep returning HTTP 200 with the plaintext input/output and the full history, where AWS refuses both with KmsInvalidStateException (HTTP 400) and the relayed "<CMK ARN> is disabled. (Service: Kms, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" message.
8. kms.enable_key(KeyId=<CMK>) and confirm both reads succeed again and return the SAME input, output and history event sequence recorded in step 5 (the refusal must not latch).
