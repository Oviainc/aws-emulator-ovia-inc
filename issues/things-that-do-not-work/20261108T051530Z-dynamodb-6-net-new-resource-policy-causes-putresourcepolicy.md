Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected dynamodb.ddb-rbp-foreign-service-action-rejected dynamodb.ddb-rbp-table-action-in-a-stream-policy-rejected dynamodb.ddb-rbp-policy-over-20480-bytes-rejected dynamodb.put-resource-policy-oversized dynamodb.ddb-rbp-public-principal-policy-rejected dynamodb.ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected|_error#10|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: dynamodb.ddb-rbp-foreign-service-action-rejected|run#5|AssertionError|assert (code, message) == ("ValidationException", expected_message), (
Issue-Key: dynamodb.ddb-rbp-table-action-in-a-stream-policy-rejected|run#7|AssertionError|assert message == expected, f"unexpected message {message!r}"
Issue-Key: dynamodb.ddb-rbp-policy-over-20480-bytes-rejected|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.put-resource-policy-oversized|run#20|AssertionError|assert message == expected_message, (
Issue-Key: dynamodb.ddb-rbp-public-principal-policy-rejected|run#8|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: dynamodb.ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant|_poll_until#18|Timeout|time.sleep(2)
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# dynamodb: 6 net-new resource-policy causes — PutResourcePolicy accepts a policy naming a nonexistent principal, four refusal messages are emulator-authored, and a policy put never settles

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Six net-new root causes in DynamoDB's resource-based-policy surface (PutResourcePolicy / GetResourcePolicy on table and stream ARNs). One is an over-permissive accept of a policy real AWS refuses; four are refusal messages the emulator authors itself; one is a put that never reaches a readable state.

Cause 1 of 6: a policy naming an IAM principal that does not exist is ACCEPTED.
AWS: Smithy: PutResourcePolicy{ResourceArn (required), Policy (required)} -> errors InternalServerError, InvalidEndpointException, LimitExceededException, PolicyNotFoundException, ResourceInUseException, ResourceNotFoundException, plus the service-common ValidationException (HTTP 400). AWS validates the policy's Principal against IAM and refuses a policy whose Principal ARN names an IAM user that does not yet exist, with ValidationException and HTTP 400, attaching nothing; the identical call succeeds once that user exists.
Emulator: the call SUCCEEDS (2xx, a RevisionId is returned) and the policy is attached.
Why: this is the ordering a stack that grants a user access to a table it also creates depends on. On the emulator a template that attaches a policy before its principal exists appears to work, then behaves differently on AWS; and a grant can be attached to a principal that will never exist, so the emulator is over-permissive at the point where AWS gates it.
Fix: PutResourcePolicy's policy-document validation — the Principal-existence check.
Traces: dynamodb.ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected

Cause 2 of 6: a table policy naming a FOREIGN service's action.
AWS: ValidationException, HTTP 400, fixed text with no variable part: "One or more parameter values were invalid: Invalid policy document: Action field includes AWS services that are inconsistent with specified vendor".
Emulator: 400 ValidationException with "One or more parameter values were invalid: Invalid policy document: Policy has an action that is not supported for the resource: <action>" — a different sentence that also interpolates the offending action, which AWS does not.
Why: the message is the requirement; a caller distinguishing a foreign-vendor action from an unsupported-for-this-resource action by its text cannot.
Fix: PutResourcePolicy's policy-document validation (action-vendor check).
Traces: dynamodb.ddb-rbp-foreign-service-action-rejected

Cause 3 of 6: a table ACTION in a STREAM-ARN policy.
AWS: ValidationException, HTTP 400, template 'One or more parameter values were invalid: Invalid policy document: The following action names are invalid: "<action>"' — fixed text around a quoted action name taken from the request.
Emulator: 400 ValidationException with "One or more parameter values were invalid: Invalid policy document: Policy has an action that is not supported for the resource: dynamodb:GetItem" — the same generic sentence it uses for cause 2, so two distinct AWS error classes collapse onto one emulator message.
Why: a caller cannot tell an invalid action NAME from a foreign-vendor action, and neither text matches AWS.
Fix: PutResourcePolicy's policy-document validation (per-resource-kind action allow-list for stream ARNs).
Traces: dynamodb.ddb-rbp-table-action-in-a-stream-policy-rejected

Cause 4 of 6: a policy document over the 20480-byte limit.
AWS: ValidationException, HTTP 400, fixed text "One or more parameter values were invalid: Maximum policy size of 20480 bytes exceeded" — the limit is part of AWS's fixed wording; nothing is interpolated.
Emulator: 400 ValidationException with "One or more parameter values were invalid: Policy size exceeds the maximum allowed size of 20480 bytes" — same limit, different sentence.
Why: message fidelity; two independent checks byte-compare this text.
Fix: PutResourcePolicy's policy-size validation.
Traces: dynamodb.ddb-rbp-policy-over-20480-bytes-rejected, dynamodb.put-resource-policy-oversized

Cause 5 of 6: a policy granting Principal "*" with no condition.
AWS: ValidationException, HTTP 400, fixed text "Resource-based policy grants unbounded access in one or more elements. Please revise the policy to ensure least-privilege form of access" — note AWS does NOT prefix this with "One or more parameter values were invalid:".
Emulator: 400 ValidationException with "One or more parameter values were invalid: Invalid policy document: Policy statement must not grant access to a public principal" — a different sentence under the generic parameter-validation prefix AWS does not use for this class.
Why: the unbounded-access refusal is a distinct, documented class; a caller matching AWS's least-privilege wording never matches, and the emulator's prefix misclassifies it as ordinary parameter validation.
Fix: PutResourcePolicy's policy-document validation (public-principal / unbounded-access check) and its message.
Traces: dynamodb.ddb-rbp-public-principal-policy-rejected

Cause 6 of 6: a resource-policy put never settles into a readable grant.
AWS: recorded behavior only — a PutResourcePolicy is asynchronous: GetResourcePolicy answers PolicyNotFoundException for about 2 s after the put returns a RevisionId, and a further put or delete on the same resource answers ResourceInUseException for about 15 s; both windows then close and the grant becomes readable, including after its named principal is deleted (AWS lazily rewrites the deleted principal to its unique id without revoking the grant).
Emulator: the check polls those conditions and never observes them close — it is still polling when the harness cap of 180 s expires, so the emulator never reaches the settled, readable state AWS reaches in seconds.
Why: a client that puts a policy and then reads it back (the normal provision-then-verify sequence) never completes against the emulator.
Fix: the resource-policy state component shared by PutResourcePolicy/GetResourcePolicy/DeleteResourcePolicy (its settle/visibility transitions).
Traces: dynamodb.ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant

No open issue covers these. The nearest, 20261105T224454Z-dynamodb-getresourcepolicy-a-foreign-service-arn-is-refused.md, is about GetResourcePolicy's ARN-shape refusal and owns none of these checks.

## Evidence
Replay output, expected (real AWS) vs actual (emulator):

1. ddb-ltr-resource-policy-naming-a-nonexistent-principal-rejected — AssertionError: 'the call succeeded; real AWS rejects it'. PutResourcePolicy with a Principal ARN for a not-yet-created IAM user returned success instead of 400 ValidationException.

2. ddb-rbp-foreign-service-action-rejected — expected ('ValidationException', 'One or more parameter values were invalid: Invalid policy document: Action field includes AWS services that are inconsistent with specified vendor'); actual ('ValidationException', 'One or more parameter values were invalid: Invalid policy document: Policy has an action that is not supported for the resource: s3:GetObject').

3. ddb-rbp-table-action-in-a-stream-policy-rejected — expected 'One or more parameter values were invalid: Invalid policy document: The following action names are invalid: "dynamodb:GetItem"'; actual 'One or more parameter values were invalid: Invalid policy document: Policy has an action that is not supported for the resource: dynamodb:GetItem'.

4. ddb-rbp-policy-over-20480-bytes-rejected and put-resource-policy-oversized — expected 'One or more parameter values were invalid: Maximum policy size of 20480 bytes exceeded'; actual 'One or more parameter values were invalid: Policy size exceeds the maximum allowed size of 20480 bytes'.

5. ddb-rbp-public-principal-policy-rejected — expected 'Resource-based policy grants unbounded access in one or more elements. Please revise the policy to ensure least-privilege form of access'; actual 'One or more parameter values were invalid: Invalid policy document: Policy statement must not grant access to a public principal'.

6. ddb-rbp-deleted-principal-is-lazily-rewritten-to-its-unique-id-without-revoking-the-grant — 'Check timed out after 180.0s' while polling the documented settle windows (PolicyNotFoundException ~2 s, ResourceInUseException ~15 s on AWS).

## Steps to Reproduce
Replay the behavioral suite and re-run the listed dynamodb checks, or by hand against the emulator:
1. CreateTable, then PutResourcePolicy on its ARN with a Principal of arn:aws:iam::<acct>:user/<a-user-that-does-not-exist> -> expect 400 ValidationException and GetResourcePolicy still PolicyNotFoundException; observe success instead.
2. PutResourcePolicy on a table ARN with Action ["s3:GetObject"] -> compare the message.
3. Enable streams, then PutResourcePolicy on LatestStreamArn with Action ["dynamodb:GetItem"] -> compare the message; the control (GetRecords/DescribeStream on the same stream ARN) must be accepted.
4. PutResourcePolicy with a document just over 20480 bytes -> compare the message.
5. PutResourcePolicy with Principal "*" and no Condition -> compare the message.
6. PutResourcePolicy, then poll GetResourcePolicy until it serves the policy, and a second put until it stops answering ResourceInUseException -> both must close within seconds.
