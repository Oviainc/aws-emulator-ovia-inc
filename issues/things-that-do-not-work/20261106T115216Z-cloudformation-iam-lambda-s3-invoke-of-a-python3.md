Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive
Issue-Key: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive|_invoke#8|Timeout|return json.loads(resp["Payload"].read())
Issue-Commit: b0d768b589014546a522750598bcb5739ddb6c75

# cloudformation__iam__lambda__s3: Invoke of a python3.12 function whose handler uses the AWS SDK never returns a successful result, so a wildcard-s3 role check never converges and burns its whole budget

> _Note: this issue was identified against the code base at commit `b0d768b589014546a522750598bcb5739ddb6c75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a Lambda function whose handler performs S3 calls through the AWS SDK never produces a success payload on Invoke, so an invoke-until-success loop never converges. Failing check: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive.

AWS: recorded behavior only for the end-to-end arm (no single modeled constraint covers it). The check creates two buckets, an IAM role trusting lambda.amazonaws.com, an inline role policy {"Effect":"Allow","Action":"s3:*","Resource":"*"}, and a python3.12 function with Handler=lambda_function.handler and Timeout=30. On AWS: GetFunction reports State=Active within seconds; Invoke returns 200 with the handler's JSON payload; the managed python3.12 runtime bundles the AWS SDK (boto3), so `import boto3` succeeds; and because the execution role allows s3:* on *, the handler's PutObject into bucket A and GetObject from bucket B both succeed, making the handler return {"ok": true} (and {"ok": true, "body": <content>} for the read). The whole sequence completes well inside the harness's 60s per-check budget, and the object written by the handler is then readable with S3 GetObject by the caller.

Emulator: no Invoke response ever satisfies ok==true. The check's retry helper re-invokes every 4s for up to 90s, so the check is killed by the harness: `Error: Check timed out after 60.0s`. The grant is deliberately generic (Action "s3:*", Resource "*", two independently created buckets) precisely so a per-bucket-ARN special case cannot pass it. Hypothesis: this is the same runtime gap already reported in open issue 20261105T085348Z-cloudformation-iam-lambda-s3-2-net-new-root.md ('the Lambda python runtime has no AWS SDK') — with no boto3 in the runtime the handler's except-arm returns {"ok": false, "error": "ModuleNotFoundError"} forever; that issue does not carry this check's replay key, so this check is filed here. The second possibility, which the same evidence allows, is that the emulator's IAM evaluation for the function's S3 calls does not honour a wildcard Action/Resource statement generically and answers AccessDenied, giving the same never-ok payload.

Why: a client that packages a handler using the AWS SDK and grants it broad S3 access — the single most common Lambda shape — gets no usable result from Invoke at all. Nothing distinguishes 'the code failed' from 'the permission was denied' from 'the platform has no SDK', and every poll-until-success pattern built on Invoke hangs instead of failing fast, so the data-plane effect (the object the handler was supposed to write) never lands.

Fix: the Lambda Invoke execution path — the runtime image/environment the python3.12 handler is executed in (it must provide the AWS SDK that the managed runtime provides, with credentials for the function's execution role), and the authorization of the SDK calls the handler makes, which must evaluate wildcard Action/Resource statements on the execution role generically rather than against specific bucket ARNs. The handler's return value must be surfaced as the Invoke payload either way, so an authorization refusal shows up as the handler's own error payload rather than as an unending retry.

Traces: cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive (put into bucket A then get from bucket B through the same wildcard-granted role).

## Evidence
Failing check cloudformation__iam__lambda__s3.lambda-role-wildcard-resource-s3-access-positive [mismatch]:

  Error: Check timed out after 60.0s

The check's shape, from the replayed source: `_invoke_until(ctx, fn, {"action": "put", ...}, lambda r: r.get("ok") is True)` re-invokes every 4s up to 90s and returns the last payload; the check is therefore killed by the 60s harness budget only if no Invoke ever returns {"ok": true}.

Expected (AWS): create_function(Runtime="python3.12", Handler="lambda_function.handler") becomes Active; the first Invoke with payload {"action":"put","bucket":"<bucket-a>","key":"a.txt","content":"<content>"} returns {"ok": true}; S3 GetObject on bucket A/a.txt then returns that content; and the Invoke with {"action":"get","bucket":"<bucket-b>","key":"b.txt"} returns {"ok": true, "body": "<content>"}. Bucket names, keys and content are per-run values from the request, not part of the requirement.
Actual (emulator): no Invoke in ~60s of retries returns ok=true, so the check never reaches its first assertion and is terminated by the budget. The handler's own except-arm would report the underlying failure as {"ok": false, "error": <code>}, so the failure is in the invoke/runtime or in the authorization of the handler's S3 calls, not in the check.

## Steps to Reproduce
1. Create two buckets with s3 CreateBucket (any two distinct names; add the LocationConstraint outside us-east-1).
2. iam CreateRole with an assume-role policy allowing sts:AssumeRole for Service lambda.amazonaws.com.
3. iam PutRolePolicy on that role with {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}.
4. lambda CreateFunction with Runtime=python3.12, Handler=lambda_function.handler, Timeout=30, Role=<that role ARN>, and a ZipFile containing a lambda_function.py that does `import boto3`, then s3.put_object / s3.get_object according to the event and returns {"ok": True} / {"ok": True, "body": ...}, catching exceptions into {"ok": False, "error": <code>}.
5. Wait for GetFunction to report Configuration.State == Active.
6. lambda Invoke with Payload {"action":"put","bucket":"<bucket-a>","key":"a.txt","content":"wild-a"}. Expected payload {"ok": true}; the emulator never returns ok=true (re-invoking every 4s), so the enclosing check hits its 60s budget.
7. s3 PutObject "wild-b" at <bucket-b>/b.txt and Invoke with {"action":"get","bucket":"<bucket-b>","key":"b.txt"}: expected {"ok": true, "body": "wild-b"}.
