Issue-Severity: MEDIUM
Issue-Commit: fc759f6a3a0a0e1d24e16f577daa39145fc82623

# CloudTrail records requestParameters on a REFUSED iam:CreateUser instead of null (1 cause)

> _Note: this issue was identified against the code base at commit `fc759f6a3a0a0e1d24e16f577daa39145fc82623`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: the audit record CloudTrail serves for an IAM call that was REFUSED for want of permission carries the request's parameters in `requestParameters`, where real AWS records `null`.

AWS: recorded behavior only — the Smithy model describes `Event.CloudTrailEvent` merely as "A JSON string that contains a representation of the event returned" and says nothing about the record's members, so the requirement comes from the check's MEASURED note taken from real AWS (account pool, us-west-2 caller, read back from the delivered us-east-1 record): for an `iam:CreateUser` refused with errorCode `AccessDenied`, BOTH `requestParameters` and `responseElements` in the delivered record are `null`. The general rule being asserted is that a call refused by authorization is audited with no request parameters and no response elements at all — the parameters are only recorded for a call that was actually authorized and dispatched. The contrast the check draws is the SAME session's ALLOWED `iam:GetRole`, whose record DOES carry `requestParameters` (`{"roleName": <the role named in the request>}`) — the name there is a value from the request, not part of the requirement.

Emulator: the refused `CreateUser`'s record is produced, and is produced correctly up to this point — errorCode is `AccessDenied`, the errorMessage names `iam:CreateUser` and the assumed-role session ARN and is byte-identical to the message the SDK received, and `responseElements` is already `null`. Only `requestParameters` diverges: the replay shows it populated with the request's own input, `{'userName': '<the user name the refused CreateUser named>'}` (the literal name in the replay output is a per-run generated value, not a requirement). The assertion that fails is `requestParameters is None`, reported as "requestParameters is {'userName': ...} on a REFUSED call; MEASURED on real AWS it is null".

Why: a client or auditor that reads delivered CloudTrail records to distinguish authorized-and-dispatched calls from calls stopped at authorization gets the wrong answer — a refusal looks like a call whose parameters reached the service. Tooling that treats a non-null `requestParameters` as evidence the request body was accepted will mis-report denials, and any redaction/least-disclosure expectation about denied requests is broken because the emulator echoes the denied request's input into the audit trail. It also breaks the allowed-vs-denied contrast that monitoring rules are written against (present on success, absent on denial).

Fix: the component that renders the CloudTrail record for a refused call — the audit-record emission path that builds `requestParameters`/`responseElements` for the event. It must suppress `requestParameters` (emit JSON `null`, matching how `responseElements` is already suppressed) whenever the audited call was refused, i.e. whenever the record carries an `errorCode`, and keep populating it for calls that were authorized and dispatched.

Traces: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session (mismatch; assertion at the refused record's `requestParameters`). No open issue covers this: the nearby open CloudTrail issues concern records that are never produced at all (e.g. 20261031T171713Z-cloudtrail-audit-records-are-attributed-to-the-ambient.md, 20261029T173328Z-cloudtrail-records-and-delivers-no-audit-record-for.md, 20261030T124208Z-cloudtrail-records-no-iam-amazonaws-com-getuser-event.md) and the two `requestParameters` issues found are STS/sourceIdentity ones (20261028T010425Z, 20261028T010530Z); here the record IS produced and is wrong in one member.

## Evidence
Failing check: cloudtrail__iam.obs-iam-1-denied-create-user-is-audited-as-access-denied-naming-the-session [mismatch]

Replay output:
  File "check_cloudtrail__iam_obs_iam_1_denied_create_user_is_audited_as_access_denied_naming_the_session.py", line 324, in run
    assert denied_rec["requestParameters"] is None, (
  AssertionError: requestParameters is {'userName': 'sim2real-behav-obsiam1du-d5684d90a0da'} on a REFUSED call; MEASURED on real AWS it is null

Expected (real AWS, per the check's MEASURED header): for the refused iam:CreateUser the delivered us-east-1 record has requestParameters == null and responseElements == null, errorCode "AccessDenied", an errorMessage byte-identical to the SDK's and naming both "iam:CreateUser" and the assumed-role session ARN arn:aws:sts::<acct>:assumed-role/<role>/<session>.

Actual (emulator): errorCode, errorMessage and responseElements==null all matched — the check passed every one of those assertions — and the run failed only on requestParameters, which echoes the denied request's input member `userName`. The quoted user name is a per-run generated value; the requirement is the null, not that string.

Contrast asserted immediately after in the same check (the positive control): the same session's ALLOWED iam:GetRole record must carry requestParameters == {"roleName": <role named in the request>}, so the member must not be suppressed unconditionally.

## Steps to Reproduce
1. Create an IAM role whose inline policy grants only iam:GetRole, and assume it (session name of your choosing) to get a limited session.
2. With the limited session, call iam:GetRole on that role — it is allowed (positive control).
3. With the same limited session, call iam:CreateUser with a fresh user name. It is refused; capture the SDK's error code (AccessDenied), message and RequestId.
4. Poll CloudTrail LookupEvents in us-east-1 (the partition leader region for IAM) with LookupAttributes EventName=CreateUser over a window covering the call, and parse the CloudTrailEvent JSON of the record naming that role.
5. Inspect the parsed record: requestParameters is {'userName': <the name sent in step 3>}. Real AWS reports null there (and null responseElements, which the emulator already does).
6. For the contrast, do the same lookup with EventName=GetRole for the allowed call: that record must still carry requestParameters {'roleName': <role name>}.
