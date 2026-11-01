Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session
Issue-Key: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session|run#55|AssertionError|assert denied_rec["requestParameters"] is None, (
Issue-Commit: f5bcb0a946788170dc0fa0f931c1e9072e8c121c

# CloudTrail audit record for a REFUSED iam:CreateUser carries requestParameters instead of null (1 cause)

> _Note: this issue was identified against the code base at commit `f5bcb0a946788170dc0fa0f931c1e9072e8c121c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
This finding is grounded in the single surviving failing check of this sweep (every other failure in the report is marked ALREADY FILED or OWNED). An open issue with the same root cause already exists — 20261031T211333Z-cloudtrail-iam-cloudtrail-records-requestparameters-on-a-refused-iam-createuser.md — but the replay report did NOT mark this check as OWNED by it, i.e. that issue does not carry this check's replay key, so the check is re-filed here WITH its check id in trace_ids. A fixer should treat the two as one work item and close them together.

Cause 1 of 1: the CloudTrail audit record served for an IAM call that was REFUSED by authorization echoes the denied request's input members in `requestParameters`, where real AWS records `null`.

AWS: recorded behavior only. Smithy (cloudtrail-2013-11-01, shape `Event`, member `CloudTrailEvent`) documents the record merely as "A JSON string that contains a representation of the event returned" and models nothing about the record's members, so the requirement comes from the check's MEASURED-on-real-AWS header (account pool, us-west-2 caller, read back from the delivered us-east-1 record): for an `iam:CreateUser` refused for want of permission, the delivered record carries errorCode `AccessDenied`, an `errorMessage` byte-identical to the SDK's (naming both the fixed text `iam:CreateUser` and the assumed-role session ARN `arn:aws:sts::<account>:assumed-role/<role>/<session>`, where account/role/session are values from the request and state), and BOTH `requestParameters` AND `responseElements` are `null`. The general rule: a call stopped at authorization is audited with no request parameters and no response elements; `requestParameters` is populated only for a call that was actually authorized and dispatched. The contrast the same check asserts is the SAME session's ALLOWED `iam:GetRole`, whose record DOES carry `requestParameters` = {"roleName": <the role named in the request>} — so the member must not be suppressed unconditionally.

Emulator: the refused CreateUser's record IS produced and is correct up to this member — the check passes the errorCode (`AccessDenied`), the errorMessage identity/content assertions, and `responseElements is None`. It then fails on `assert denied_rec["requestParameters"] is None` with "AssertionError: requestParameters is {'userName': 'sim2real-behav-obsiam1du-5e6377be012b'} on a REFUSED call; MEASURED on real AWS it is null". The emulator populates `requestParameters` with the refused request's own input member `userName`. The quoted user name is a per-run generated value filling the template; the requirement is the `null`, not that string.

Why: a client or auditor that reads delivered CloudTrail records to separate authorized-and-dispatched calls from calls stopped at authorization gets the wrong answer — a denial looks like a request whose body reached the service. Monitoring rules written against the documented allowed-vs-denied contrast (parameters present on success, absent on denial) misfire, and the least-disclosure expectation for denied requests is broken because the denied input is echoed into the audit trail.

Fix: in the CloudTrail audit-record emission path — the component that renders `requestParameters`/`responseElements` for an event. It must suppress `requestParameters` (emit JSON `null`, exactly as `responseElements` is already suppressed) whenever the audited call was refused, i.e. whenever the record carries an `errorCode`, while continuing to populate it for calls that were authorized and dispatched.

Traces: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session (mismatch; assertion on the refused record's `requestParameters`, check source line 324 / report lines 16176-16183 and 16509-16512).

## Evidence
Failing check: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session [mismatch]

Replay output:
  File "check_cloudtrail__iam_obs_iam_1_denied_create_user_is_audited_as_access_denied_naming_the_session.py", line 324, in run
    assert denied_rec["requestParameters"] is None, (
  AssertionError: requestParameters is {'userName': 'sim2real-behav-obsiam1du-5e6377be012b'} on a REFUSED call; MEASURED on real AWS it is null

Expected (real AWS, per the check's MEASURED header): the delivered us-east-1 record for the refused iam:CreateUser has requestParameters == null and responseElements == null, errorCode "AccessDenied", eventSource "iam.amazonaws.com", eventName "CreateUser", awsRegion "us-east-1" (IAM is global; its events are filed in the partition leader region whatever region signed the call), readOnly false, and an errorMessage byte-identical to the SDK's message, naming the fixed text "iam:CreateUser" and the session ARN arn:aws:sts::<account>:assumed-role/<role>/<session>.

Actual (emulator): errorCode, errorMessage (identity with the SDK message, naming the action and the session ARN) and responseElements == null all matched — the check passed every one of those assertions — and the run failed only on requestParameters, which is the denied request's own input, {'userName': <the name the refused CreateUser sent>}.

Contrast asserted immediately after in the same check (positive control, must keep working): the same session's ALLOWED iam:GetRole record carries requestParameters == {"roleName": <role named in the request>}.

Model citation: Smithy cloudtrail-2013-11-01, Event.CloudTrailEvent — "A JSON string that contains a representation of the event returned"; the model constrains no member of that JSON, hence "recorded behavior only".

## Steps to Reproduce
1. Create an IAM role with an inline policy granting ONLY iam:GetRole, trusting the account root, and assume it with sts:AssumeRole to obtain a limited session.
2. With the limited session, call iam:GetRole on that role — allowed; this is the positive control.
3. With the same limited session, call iam:CreateUser with a fresh user name. It is refused: capture the SDK error code (AccessDenied), the message and the RequestId.
4. Poll CloudTrail LookupEvents in us-east-1 (the aws-partition leader region for IAM) with LookupAttributes EventName=CreateUser over a window covering the call, and json-parse the CloudTrailEvent of the record naming that role.
5. Inspect the parsed record: the emulator reports requestParameters = {'userName': <the name sent in step 3>}. Real AWS reports null there (and null responseElements, which the emulator already does).
6. Repeat the lookup with EventName=GetRole for the allowed call and confirm that record still carries requestParameters {'roleName': <role name>} — the suppression must apply only to records carrying an errorCode.
