Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one|_poll#17|Timeout|time.sleep(interval_s)
Issue-Commit: 7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569

# secretsmanager: a scheduled-but-unfired rotation never runs and CancelRotateSecret leaves no observable state, so both cancel-rotation checks hang to the harness cap

> _Note: this issue was identified against the code base at commit `7edae7e43dd2ef932ae2dbfd8dbf1e7a775f7569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelRotateSecret

## Description
One cause.

Cause 1 of 1: the rotation state machine never advances, so neither of the CancelRotateSecret contracts can be observed — the scheduled-but-unfired rotation never produces the state a cancel has to clear, and the stuck-token resumption after a cancel never happens. Both checks run out the 60 s harness cap with no verdict.

AWS: Smithy secretsmanager models RotateSecret {SecretId, ClientRequestToken, RotationLambdaARN, RotationRules, RotateImmediately} and CancelRotateSecret {SecretId} -> {ARN, Name, VersionId}. CancelRotateSecret turns rotation off for the secret (DescribeSecret's RotationEnabled becomes false and NextRotationDate is cleared) but does NOT undo a rotation already in flight: the candidate version stays labelled AWSPENDING, and the next RotateSecret reattempts THAT token rather than the ClientRequestToken just requested, refusing a fresh one with InvalidRequestException and the fixed message "A previous rotation isn't complete. That rotation will be reattempted." A rotation scheduled but not yet fired has no AWSPENDING candidate at all, so cancelling it only clears the schedule.

Emulator: both checks hang: nothing they poll (the rotation function's step records, DescribeSecret's VersionIdsToStages/RotationEnabled/NextRotationDate transitions, the resumed token) ever changes. Hypothesis: RotateSecret is accepted without ever invoking the rotation function or staging a candidate version, so there is no in-flight rotation for CancelRotateSecret to interact with. Several open issues record neighbouring symptoms of the same missing machinery for OTHER checks and do not carry these two: 20261028T061500Z-secretsmanager-rotatesecret-rotateimmediately-true-never-runs-the-rotation.md, 20261028T011946Z-secretsmanager-rotatesecret-with-rotateimmediately-true-never-executes-the.md and 20261030T010250Z-secretsmanager-rotatesecret-after-restoresecret-is-accepted-but-never.md.

Why: a client cannot manage rotation at all — it cannot tell whether a cancel stopped a schedule or abandoned an in-flight rotation, it never sees the AWSPENDING candidate a cancel is defined to leave behind, and the "previous rotation isn't complete" guard that forces the stuck token to be reattempted never fires, so a secret can be driven into a state AWS forbids.

Fix: the RotateSecret/CancelRotateSecret handlers and the secret's rotation state machine — RotateSecret must stage a candidate as AWSPENDING and invoke the rotation function per step, CancelRotateSecret must clear only the schedule (RotationEnabled false, NextRotationDate cleared) while leaving an in-flight token in place, and the next RotateSecret must reattempt that token and refuse a new ClientRequestToken with InvalidRequestException "A previous rotation isn't complete. That rotation will be reattempted."

Traces: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation, secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one.

## Evidence
From the behavioral replay report (suite simparity-behavioral-aws-v1, service secretsmanager 104/191):

[mismatch] secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation
Error: Check timed out after 60.0s

[mismatch] secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one
Error: Check timed out after 60.0s
(the check pins the expected refusal text in its source: _STUCK_MESSAGE = "A previous rotation isn't complete. That rotation will be reattempted." and drives a four-step rotation function whose FAIL_STEP leaves the candidate stuck.)

Expected: a rotation requested with a rotation function produces per-step records and an AWSPENDING candidate within seconds; CancelRotateSecret then returns {ARN, Name, VersionId} and DescribeSecret shows RotationEnabled false with the candidate still AWSPENDING; the next RotateSecret reattempts the stuck token and refuses a new ClientRequestToken with InvalidRequestException and the message above.
Actual: no rotation effect is ever produced, so both checks block until the harness cap.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. lambda: CreateFunction for a rotation handler (a no-op is enough for the schedule case; one that fails a named step for the stuck case) and AddPermission for secretsmanager.amazonaws.com.
3. secretsmanager: CreateSecret, then RotateSecret(SecretId=…, RotationLambdaARN=…, RotationRules={'ScheduleExpression': 'rate(10 days)'}, RotateImmediately=False) -> DescribeSecret should report RotationEnabled true with a NextRotationDate and NO AWSPENDING version.
4. CancelRotateSecret(SecretId=…) -> expect {ARN, Name, VersionId} and DescribeSecret with RotationEnabled false and NextRotationDate cleared.
5. For the stuck case: RotateSecret with RotateImmediately=True against a function that fails a step, CancelRotateSecret, then RotateSecret with a NEW ClientRequestToken -> expect InvalidRequestException "A previous rotation isn't complete. That rotation will be reattempted." and the original token reattempted.
6. Replay the two checks in trace_ids; both currently block to the 60 s cap.
