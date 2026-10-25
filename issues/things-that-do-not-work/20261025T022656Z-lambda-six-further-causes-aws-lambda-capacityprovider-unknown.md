Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.deploy-capacity-provider-cloudformation-scaling-behavior lambda.deploy-lambda-1-version-alias-code-update lambda.deploy-lambda-alias-weight-on-primary-version-rejected lambda.concurrency-throttles-at-limit lambda.thirty-way-concurrency-creates-thirty-distinct-environments lambda.tmp-not-shared-between-concurrent-invocations lambda.cli-script-lambda-deploy-and-invoke lambda.e2e-sqs-trigger lambda.function-url-response-stream-mode-chunks-over-https lambda.list-functions-throttling-and-pagination-consistency
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateStack|ValidationError
Issue-Key: lambda.deploy-lambda-1-version-alias-code-update|run#15|AssertionError|assert alias_arn.endswith(f":{function_name}:live"), (
Issue-Key: lambda.deploy-lambda-alias-weight-on-primary-version-rejected|run#13|IndexError|v1_number = _outputs(stack)["Version1Arn"].rsplit(":", 1)[1]
Issue-Key: lambda.concurrency-throttles-at-limit|run#13|AssertionError|assert warm == ("ok", 200), (
Issue-Key: lambda.thirty-way-concurrency-creates-thirty-distinct-environments|run#14|AssertionError|assert not throttled1, (
Issue-Key: lambda.tmp-not-shared-between-concurrent-invocations|run#12|AssertionError|assert not isinstance(res, BaseException), f"invocation {marker} raised {res!r}"
Issue-Key: lambda.cli-script-lambda-deploy-and-invoke|_run_shell#3|AssertionError|raise AssertionError(
Issue-Key: lambda.e2e-sqs-trigger|run#15|Timeout|time.sleep(2)
Issue-Key: lambda.function-url-response-stream-mode-chunks-over-https|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.list-functions-throttling-and-pagination-consistency|run#16|AssertionError|assert len(codes) >= 1, (
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda: six further causes — AWS::Lambda::CapacityProvider unknown to CloudFormation, alias Ref is not an ARN, empty version list, and invoke-dependent behaviours never observable

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Six net-new root causes grouped for the lambda service.

Cause 1 of 6 — CloudFormation refuses AWS::Lambda::CapacityProvider as an unrecognized resource type.
AWS: the type is provisionable; CreateStack accepts the template and the capacity provider is created in Lambda, so its scaling behaviour can be driven from a stack. Recorded behavior only for the exact wording. Emulator: CreateStack is refused 400 'ValidationError' with 'Template format error: Unrecognized resource types: [AWS::Lambda::CapacityProvider]' (recorded message, an example). Why: capacity providers cannot be managed as code, and the error blames the client's template instead of naming an unsupported type. Fix: the CloudFormation resource-type registry and a provisioner bound to the lambda capacity-provider handlers. Traces: lambda.deploy-capacity-provider-cloudformation-scaling-behavior.

Cause 2 of 6 — Ref of an AWS::Lambda::Alias does not return the alias ARN.
AWS: Ref on AWS::Lambda::Alias returns the alias ARN (arn:aws:lambda:<region>:<account>:function:<fn>:<alias>), which is what downstream resources and stack outputs consume. Emulator: the value the stack exposes for the alias is not that ARN. Why: anything that consumes the Ref (an event source, a permission, a stack output a client invokes) gets an identifier Lambda will not accept. Fix: the AWS::Lambda::Alias provisioner's Ref/physical-id mapping in the CloudFormation resource handlers. Traces: lambda.deploy-lambda-1-version-alias-code-update.

Cause 3 of 6 — a version/alias-routing read returns an empty list.
AWS: after PublishVersion, the function's version list contains the published version, so a client can select the primary version to weight an alias against; weighting an alias at its own primary version is then refused InvalidParameterValueException (400). Emulator: the list the check indexes is empty, so the call fails before the refusal can be reached (IndexError). Why: a client cannot enumerate its own published versions, and the alias weighting refusal is unobservable. Fix: the ListVersionsByFunction/alias routing-config read in the lambda service. Traces: lambda.deploy-lambda-alias-weight-on-primary-version-rejected.

Cause 4 of 6 — concurrency admission control and environment isolation are unobservable because Invoke never runs the package.
AWS: with ReservedConcurrentExecutions=N, a burst of N concurrent invocations all succeed (none throttled) and a solo invoke under an active limit succeeds; each concurrent invocation gets its own /tmp, and the AWS CLI deploy-and-invoke script returns the handler's payload. Emulator: every invocation is refused 'NotImplemented' ('running its <runtime> deployment package is not implemented in this emulator'), so a 30-wide burst produces 30 rejections, a solo invoke is reported throttled/NotImplemented, /tmp isolation cannot be probed and the CLI script exits 254. Why: the client sees a non-modeled error for valid invocations and cannot verify any concurrency or isolation guarantee. Fix: the lambda Invoke execution path (a runtime that executes the deployment package) and the admission-control accounting layered on it. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md records the underlying refusal but lists none of these four checks. Traces: lambda.concurrency-throttles-at-limit, lambda.thirty-way-concurrency-creates-thirty-distinct-environments, lambda.tmp-not-shared-between-concurrent-invocations, lambda.cli-script-lambda-deploy-and-invoke.

Cause 5 of 6 — two invoke-path checks never complete and are killed at the harness 60 s cap.
AWS: an SQS-triggered function is invoked within the poller's delay, and a function URL configured with InvokeMode=RESPONSE_STREAM streams chunks over HTTPS as the handler writes them. Recorded behavior only for the timing. Emulator: neither terminates — 'Error: Check timed out after 60.0s'. Why: callers and waiters hang rather than getting a response or an error, which no client can time out of correctly. Fix: the SQS event-source-mapping poller and the function-URL RESPONSE_STREAM data path in the lambda service. Related open issue 20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md covers function-URL routing but lists neither check. Traces: lambda.e2e-sqs-trigger, lambda.function-url-response-stream-mode-chunks-over-https.

Cause 6 of 6 — control-plane calls are never throttled.
AWS: the account's shared control-plane quota makes at least some of a 500-wide concurrent ListFunctions burst fail with TooManyRequestsException (429) and a Retry-After, which is the signal SDK retry/backoff depends on; pagination stays consistent across the burst. Recorded behavior only for the exact threshold. Emulator: all 500 calls succeed, so no client-side retry path is ever exercised. Why: code that must back off is never told to, and a client cannot distinguish an emulator with no quota from an account with a huge one. Fix: control-plane request accounting/throttling in the lambda service's request path. Traces: lambda.list-functions-throttling-and-pagination-consistency.

## Evidence
Cause 1: ClientError (ValidationError) ... Template format error: Unrecognized resource types: [AWS::Lambda::CapacityProvider].
Cause 2: "AssertionError: alias Ref 'live' is not the ARN of alias 'live' on 'sim2real-cfnva-fn-...'".
Cause 3: IndexError: list index out of range.
Cause 4: "a 30-wide burst against ReservedConcurrentExecutions=30 must not be throttled, got 30 rejections: ['NotImplemented', ...]"; "a solo invoke must succeed under a still-active ReservedConcurrentExecutions=1, got ('throttled','NotImplemented')"; 'invocation sim2real-tmp-a raised ClientError(... NotImplemented ...)'; 'Command failed (rc=254)'.
Cause 5: Error: Check timed out after 60.0s (both checks).
Cause 6: 'expected at least one of 500 concurrent ListFunctions calls to be throttled given the account's shared control-plane quota'.

## Steps to Reproduce
1. CreateStack with an AWS::Lambda::CapacityProvider resource.
2. CreateStack with AWS::Lambda::Function + AWS::Lambda::Alias and read the alias Ref/stack output.
3. PublishVersion, then list versions and try to weight the alias at its primary version.
4. Set ReservedConcurrentExecutions and issue a burst of concurrent Invoke calls; run the AWS CLI deploy-and-invoke script.
5. Wire an SQS queue to a function via an ESM and send a message; create a function URL with InvokeMode=RESPONSE_STREAM and GET it.
6. Issue 500 concurrent ListFunctions calls.
