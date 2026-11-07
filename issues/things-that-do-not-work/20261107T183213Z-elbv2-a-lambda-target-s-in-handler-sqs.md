Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target|_collect#9|AssertionError|assert len(bodies) >= want, (
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# elbv2: a Lambda target's in-handler sqs:SendMessage never produces a message after lambda:Invoke, so the desync-mitigation data-plane check cannot reach the ALB legs it exists to measure

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause (1 of 1) behind the one surviving unmarked elbv2 failure.

Related open filings that do NOT carry this replay key: 20261107T080944Z-elbv2-2-net-new-data-plane-root-causes.md and 20261105T093535Z-elbv2-4-net-new-data-plane-causes-desync.md own other elbv2 data-plane checks (the latter names the desync family but not this check's replay key), and the open lambda execution-environment filings (e.g. 20261107T135301Z-lambda-10-net-new-root-causes-in-the--r2.md, 20261107T091747Z-lambda-the-execution-environment-has-no-filesystem-or.md) describe the sandbox generally without carrying this check. Filing under the service heading the suite reports the check under (elbv2); the defect itself is in the Lambda invocation path the elbv2 Lambda-target scenario depends on.

Cause 1 of 1: a synchronous lambda:Invoke of a function whose handler calls sqs:SendMessage returns, but no message ever arrives on the queue.
AWS: recorded behavior only (the side channel is the function's own code, not a modeled shape). A synchronous Invoke runs the handler to completion before responding; a boto3 sqs.send_message the handler performs against a queue its execution role is allowed to write is committed by the time the Invoke response is returned, so a subsequent ReceiveMessage on that queue returns the body the handler sent — here a JSON object carrying the request's path ('/direct') and the invocation's aws_request_id.
Emulator: the Invoke returns, but polling the queue for 60 s yields nothing: `AssertionError: the target reported 0 invocation(s) on the side channel within 60s, expected 1; bodies=[]`. This fails at the check's FIRST step — the pre-ALB control that proves the artifact and its side channel work before any load balancer exists (check lines 77771-77777) — so the emulator is never asked anything about desync mitigation at all.
Why: two distinct losses. (a) A client cannot rely on work a handler performs during a synchronous Invoke having happened when the call returns — the central guarantee of synchronous invocation; an enqueue that silently vanishes turns a completed Invoke into a lost message with no error anywhere. (b) Because the side channel is how a Lambda TARGET reports whether the ALB forwarded a request to it, every ALB-to-Lambda behavior that counts invocations is unmeasurable: here, that desync_mitigation_mode='strictest' answers an ambiguously framed request (Content-Length plus Transfer-Encoding: chunked) with 400 and does NOT invoke the target, while the identical bytes under 'monitor' do reach it, and a compliant POST is forwarded in both modes. None of that is exercised.
Fix: the Lambda invocation path in the lambda service crate — the synchronous Invoke must execute the handler and commit the AWS calls it makes (here sqs:SendMessage through the function's execution role) before responding, so in-handler side effects are observable to the caller afterwards.
Traces: elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 202/554 passed; elbv2 150/294 (51%), 134 failed (10 unloadable). 133 of the 134 are marked OWNED/ALREADY FILED; this one is not.

[mismatch] elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target:
  File "check_elbv2_desync_mitigation_strictest_blocks_ambiguous_framing_before_the_target.py", line 553, in run
    direct, _ = _collect(ctx, queue_url, want=1, timeout_s=60)
  File "...", line 313, in _collect
    assert len(bodies) >= want, ...
  AssertionError: the target reported 0 invocation(s) on the side channel within 60s, expected 1; bodies=[]

Check source at the failing step (report lines 77771-77777): after creating the SQS queue, a VPC/SG, an execution role granting sqs:SendMessage and the function, the check does `lam.invoke(FunctionName=fn['name'], Payload=json.dumps({'path': '/direct', 'httpMethod': 'POST'}))` and then requires exactly one message whose JSON 'path' is '/direct'. The handler (report lines 77713-77724) sends that JSON to QUEUE_URL and returns {'statusCode': 200, 'body': 'TARGET-REACHED'}. Expected: one message on the queue. Actual: none within 60 s, so the ALB legs (lines 77795+: desync mode 'monitor' forwards both the compliant and the ambiguous request; mode 'strictest' answers the ambiguous one 400 and invokes nothing) are never reached. The queue URL, function name and request id are per-run values.

## Steps to Reproduce
1. Create an SQS queue and an IAM role allowing sqs:SendMessage to it; create a Python Lambda function with QUEUE_URL in its environment whose handler does boto3 sqs.send_message(QueueUrl=os.environ['QUEUE_URL'], MessageBody=json.dumps({'path': event['path'], 'request_id': context.aws_request_id})) and returns {'statusCode': 200, 'body': 'TARGET-REACHED'}.
2. lambda.invoke(FunctionName=<fn>, Payload=b'{"path": "/direct", "httpMethod": "POST"}') (synchronous, RequestResponse).
3. sqs.receive_message on the queue for up to 60 s: AWS returns one message whose JSON 'path' is '/direct'; the emulator returns none.
4. With that control fixed, continue the scenario: create an internet-facing ALB, a TargetType='lambda' target group registering the function, an HTTP:80 listener forwarding to it, then set desync_mitigation_mode='monitor' and 'strictest' and send a compliant POST and an ambiguously framed POST (Content-Length: 8 together with Transfer-Encoding: chunked) on a raw socket, counting invocations on the queue.
5. Re-replay elbv2.desync-mitigation-strictest-blocks-ambiguous-framing-before-the-target.
