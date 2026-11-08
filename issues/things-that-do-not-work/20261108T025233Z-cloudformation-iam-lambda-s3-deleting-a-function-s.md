Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative
Issue-Key: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative|_invoke_until_denied#29|AssertionError|raise AssertionError(
Issue-Commit: a5ed122d181173976052c81dab0291863966c94e

# cloudformation__iam__lambda__s3: deleting a function's execution role has no effect on Invoke — the invocation still succeeds and the handler's in-function S3 write lands, where AWS fails the invocation

> _Note: this issue was identified against the code base at commit `a5ed122d181173976052c81dab0291863966c94e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause. Cause count: 1 — a fixer must close this one cause before submitting.

=== Cause 1 of 1: the function's execution role is not re-resolved or re-assumed at invocation time, so an invocation after DeleteRole runs with full effect ===
Failing check: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative.
AWS: Lambda assumes the function's execution role to run the function and to vend the credentials the handler's SDK uses. Once that role is deleted (DeleteRole, after its inline policy is deleted and its managed policy detached), Lambda can no longer assume it and the invocation does NOT succeed: either the Invoke itself is refused — the CLI exits non-zero with error code AccessDeniedException (the role the function names cannot be assumed) — or, once a fresh execution environment must be created, the handler's own SDK calls fail with a credential error (InvalidAccessKeyId or UnrecognizedClientException) and the invocation reports a function error. Either way the invocation's side effect is absent: the S3 marker object for that invocation is never written. Smithy (com.amazonaws.lambda#AWSGirApiService): Invoke is POST /2015-03-31/functions/{FunctionName}/invocations with success code 200 and models no authorization error at all — its modeled errors are InvalidRequestContentException/InvalidParameterValueException (400), ResourceNotFoundException (404), ResourceConflictException (409), ResourceNotReadyException/KMS*/EC2*/InvalidZipFileException (502), RequestTooLargeException (413), TooManyRequestsException (429), ServiceException (500) and peers — so AccessDeniedException/403 is a service-common refusal and the behavior above is recorded behavior only: the recording accepts any of AccessDeniedException, InvalidAccessKeyId or UnrecognizedClientException, and demands that the marker object be absent.
Emulator: after the same sequence — delete-role-policy, detach-role-policy, delete-role, then a get-role that confirms the role is gone (non-zero exit), then UpdateFunctionConfiguration to force a fresh execution environment and a poll until LastUpdateStatus=Successful — the next Invoke still SUCCEEDS. The check's poller reports last_returncode=0 and the handler's own return value, last_code="invoke-succeeded:{'wrote': 'after-delete.txt'}": the function ran and its boto3 s3:PutObject to the bucket completed, so the deleted role was neither re-resolved nor re-assumed and the in-function credentials it backed were still honoured. The positive control before the deletion passes (invoke succeeds, before-delete.txt is written), so the only thing changed between the two invocations is the existence of the execution role.
Why: a client relying on AWS's behavior treats the execution role as the live authority for everything a function does — deleting or replacing it is the documented way to revoke a function's access, and incident runbooks depend on it taking effect on the next invocation. Against the emulator a function keeps running and keeps writing to S3 after its role is destroyed, so revocation is unobservable and every negative test of the form "break exactly one thing — the role — and prove the invocation stops" passes vacuously. The divergence is over-permissive, which is the dangerous direction: the emulator reports access that AWS has withdrawn.
Fix: the Lambda invocation path — the component that resolves the function's configured Role and mints the invocation's credentials before handing control to the runtime (the Invoke handler and the execution-role/credential provisioning it calls in the lambda service crate), not the IAM handlers: it must resolve the role by ARN at invocation time, fail the invocation when no such role exists (service-common AccessDeniedException / HTTP 403 at the API, or a credential error for the in-function SDK), and leave no side effect for the failed invocation.
Traces: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative.

Related but NOT covering this check: open 20261105T085348Z-cloudformation-iam-lambda-s3-2-net-new-root.md, open 20261106T151148Z-cloudformation-iam-lambda-s3-in-function-s3-calls.md and open 20261107T184737Z-cloudformation-iam-lambda-s3-2-net-new-causes.md all report the in-process Lambda runtime's SDK being absent or incomplete; here the handler's S3 call plainly WORKED, and the defect is that it worked without a live execution role. None of those issues carries this check.

## Evidence
Suite simparity-behavioral-aws-v1, service group `cloudformation__iam__lambda__s3` 17/36 (19 failed). This check is the only failure in that group marked NEITHER "ALREADY FILED" NOR "OWNED by".

Check cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative, verdict [mismatch]:
  File "check_cloudformation__iam__lambda__s3_cli_iam_role_deleted_breaks_lambda_invoke_negative.py", line 150, in run
  code = _invoke_until_denied(ctx, function_name, after_key, timeout_s=90.0, interval_s=3.0)
  File "...", line 398, in _invoke_until_denied
  AssertionError: invoke never failed (CLI-level or in-function credential error) within 90.0s (last_returncode=0, last_code="invoke-succeeded:{'wrote': 'after-delete.txt'}")

Expected (AWS), once the execution role is deleted and a fresh execution environment is forced:
  - `aws lambda invoke --function-name <fn>` fails — a non-zero CLI exit carrying error code AccessDeniedException, or an in-function credential error reported as InvalidAccessKeyId / UnrecognizedClientException (the check accepts any of the three);
  - GetObject(<bucket>, 'after-delete.txt') -> NoSuchKey: the rejected invocation left no side effect;
  - after the role is recreated under the same name with the same trust policy, managed attachment and inline s3:PutObject policy and the function is pointed back at it, the identical Invoke succeeds again and 'after-restore.txt' appears — proving the refusal tracked the role's existence.

Actual (emulator): the invocation after DeleteRole returns success (CLI returncode 0) and the handler's return value {'wrote': 'after-delete.txt'} shows its boto3 s3:PutObject completed, so the marker object WAS written. Observed across the check's full 90 s poll at 3 s intervals, so this is the settled behavior, not a propagation lag.

Sequencing that rules out other causes: the pre-deletion invoke is a passing positive control (before-delete.txt written while the role existed); the role's removal is confirmed by polling `aws iam get-role --role-name <role>` until it exits non-zero; and UpdateFunctionConfiguration (--description) plus a poll until LastUpdateStatus=Successful forces a new execution environment before the probing invoke.

## Steps to Reproduce
1. CreateBucket B.
2. `aws iam create-role --role-name R --assume-role-policy-document` trusting lambda.amazonaws.com for sts:AssumeRole; `aws iam attach-role-policy --role-name R --policy-arn arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole`; `aws iam put-role-policy --role-name R --policy-name s3-write-inline` allowing s3:PutObject on arn:aws:s3:::B/*.
3. `aws lambda create-function --function-name F --runtime python3.12 --role <R arn> --handler index.handler --zip-file fileb://<zip>` where the handler does s3.put_object(Bucket=os.environ['BUCKET_NAME'], Key=event['key'], Body=b'ok') and returns {'wrote': key}; wait for State=Active.
4. Invoke F with {"key": "before-delete.txt"} — succeeds and the object appears (positive control).
5. `aws iam delete-role-policy`, `aws iam detach-role-policy`, then `aws iam delete-role --role-name R`; poll `aws iam get-role --role-name R` until it exits non-zero.
6. `aws lambda update-function-configuration --function-name F --description force-new-execution-environment`; poll until LastUpdateStatus=Successful (forces a fresh execution environment).
7. Invoke F with {"key": "after-delete.txt"}, retrying for up to 90 s.
   Expected: the invocation fails — non-zero CLI exit with AccessDeniedException, or an in-function InvalidAccessKeyId / UnrecognizedClientException — and GetObject(B, 'after-delete.txt') answers NoSuchKey.
   Observed: returncode 0, handler result {'wrote': 'after-delete.txt'}, and the object present in B.
8. Recreate R with the same trust/policies, UpdateFunctionConfiguration --role <R arn>, and invoke with {"key": "after-restore.txt"}: this must succeed (it does), which is what makes step 7's success a divergence rather than a broken function.
9. Replay: cloudformation__iam__lambda__s3.cli-iam-role-deleted-breaks-lambda-invoke-negative
