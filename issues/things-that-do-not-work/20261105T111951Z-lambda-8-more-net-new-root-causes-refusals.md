Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes lambda.invoke-with-response-stream-managed-streaming lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment lambda.stream-response-chunks-map-onto-handler-writes lambda.custom-runtime-streaming-response-mode-delivers-chunks-via-invoke-with-response-stream lambda.durable-execution-kms-enforcement lambda.e2e-sqs-trigger lambda.invoke-log-result lambda.invoke-status-codes lambda.print-appears-in-logs lambda.durable-execution-name-conflict-rejected lambda.durable-execution-server-managed-append-only-history lambda.get-durable-execution-state-invalid-checkpoint-token-rejected lambda.stop-durable-execution-already-terminal-is-a-no-op lambda.invoke-payload-too-large lambda.dry-run-does-not-invoke-handler lambda.event-invoke-failure-not-surfaced lambda.env-var-size-and-reserved-key-rejected lambda.invoke-refuses-a-function-arn-from-another-region lambda.code-signing-enforce-rejects-unsigned-package-rejected lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected lambda.package-non-ascii-file-names-preserved-byte-exact lambda.sync-response-at-the-payload-limit-delivered-intact-one-byte-over-rejected lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running lambda.warm-start-reuse lambda.logging-config-json-format-filters-application-log-level lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|InvokeWithResponseStream|InvalidRuntimeException
Issue-Key: lambda.e2e-sqs-trigger|_wait_for_invocation_count#20|AssertionError|raise AssertionError(
Issue-Key: lambda.invoke-log-result|run#18|Timeout|time.sleep(2.0)
Issue-Key: lambda.invoke-status-codes|_poll_log_marker#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda.print-appears-in-logs|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.durable-execution-name-conflict-rejected|run#7|AssertionError|raise AssertionError("Invoke did not return a DurableExecutionArn; durable functions unsupported here")
Issue-Key: lambda.durable-execution-server-managed-append-only-history|run#14|KeyError|assert decoded["result"]["value"] == "sim2real-behav-durable", decoded
Issue-Key: lambda.get-durable-execution-state-invalid-checkpoint-token-rejected|run#6|KeyError|arn = resp["DurableExecutionArn"]
Issue-Key: lambda.stop-durable-execution-already-terminal-is-a-no-op|run#6|KeyError|arn = resp["DurableExecutionArn"]
Issue-Key: lambda.invoke-payload-too-large|run#11|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: lambda.dry-run-does-not-invoke-handler|run#31|AssertionError|assert bad_msg == (
Issue-Key: lambda.event-invoke-failure-not-surfaced|run#22|AssertionError|assert nf_msg == expected_nf_msg, (
Issue-Key: lambda.env-var-size-and-reserved-key-rejected|run#13|AssertionError|assert big_msg == (
Issue-Key: lambda.invoke-refuses-a-function-arn-from-another-region|_client_error#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: lambda.code-signing-enforce-rejects-unsigned-package-rejected|_client_error#22|AssertionError|raise AssertionError(
Issue-Key: lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code|run#9|AssertionError|raise AssertionError(
Issue-Key: lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: lambda.package-non-ascii-file-names-preserved-byte-exact|run#14|AssertionError|assert found == expected, (
Issue-Key: lambda.sync-response-at-the-payload-limit-delivered-intact-one-byte-over-rejected|run#9|AssertionError|assert resp.get("FunctionError") is None, f"the {_RESPONSE_LIMIT}-byte response failed: {raw[:300]!r}"
Issue-Key: lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.warm-start-reuse|run#9|AssertionError|assert any(c > 1 for c in counts[1:]), (
Issue-Key: lambda.logging-config-json-format-filters-application-log-level|run#19|KeyError|request_id = resp["ResponseMetadata"]["RequestId"]
Issue-Key: lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent|_invoke#16|KeyError|request_id = resp["ResponseMetadata"]["RequestId"]
Issue-Related: 20261105T080515Z-dynamodb-lambda-rekognition-s3-lambda-invoke-refuses-a-nodejs22-x-function-with.md
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# lambda: 8 more net-new root causes — refusals that never happen, refusal messages that differ from AWS, and invocations that never actually run

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Eight further net-new root causes in the lambda service, distinct from the runtime/data-plane set filed alongside this issue.

Cause 1 of 8 — InvokeWithResponseStream and Invoke refuse any non-python runtime with a 502 that names the emulator.
AWS: Smithy: Invoke/InvokeWithResponseStream model InvalidRuntimeException as a 502 server error, raised when the runtime itself fails — not for a runtime AWS supports. nodejs22.x is a supported managed runtime, so a correctly packaged nodejs function is invoked and returns its result.
Emulator: HTTP 502 InvalidRuntimeException with message "The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes." The open issue 20261105T080515Z-dynamodb-lambda-rekognition-s3-lambda-invoke-refuses-a-nodejs22-x-function-with.md records this for other checks but does not carry these replay keys; the message's fixed text is the whole sentence and its only variable part is the runtime identifier taken from the function's configuration.
Why: a client gets a 502 naming the emulator instead of the function's response, and response streaming cannot be exercised at all.
Fix: the Invoke/InvokeWithResponseStream handler's runtime dispatch.
Traces: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer, lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes, lambda.invoke-with-response-stream-managed-streaming, lambda.stream-handler-throw-delivers-no-bytes-and-no-error-code, lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment, lambda.stream-response-chunks-map-onto-handler-writes, lambda.custom-runtime-streaming-response-mode-delivers-chunks-via-invoke-with-response-stream, lambda.durable-execution-kms-enforcement.

Cause 2 of 8 — an accepted asynchronous (Event) invocation never runs the function, and invocation logs never reach CloudWatch Logs, so every log-backed check polls to its deadline.
AWS: InvokeAsync/Invoke with InvocationType=Event returns 202 and the function then runs; every invocation writes START/END/REPORT plus handler output to /aws/lambda/<function-name>, which the log group's streams expose within seconds.
Emulator: the 202 is returned but the completion marker never appears in /aws/lambda/<fn>, and log-backed assertions never observe any line; the harness reports "Check timed out after 60.0s" or "expected >= 1 log line(s) matching '<marker>' ... found 0".
Why: asynchronous workloads silently do nothing, and no client can observe an invocation's logs, duration or errors.
Fix: the async invocation queue/worker and the Lambda->CloudWatch Logs delivery path.
Traces: lambda.e2e-sqs-trigger, lambda.invoke-log-result, lambda.invoke-status-codes, lambda.print-appears-in-logs.

Cause 3 of 8 — durable executions are accepted but never start, and Invoke returns no DurableExecutionArn.
AWS: Smithy: Invoke's output binds DurableExecutionArn to the X-Amz-Durable-Execution-Arn response header; an Invoke carrying X-Amz-Durable-Execution-Name starts a durable execution and returns that ARN, and the execution then progresses out of RUNNING.
Emulator: the header is absent ("Invoke did not return a DurableExecutionArn"; KeyError 'DurableExecutionArn'), and executions that are created stay Status=RUNNING past the wait window.
Why: durable-function clients cannot obtain the execution ARN they need to poll, resume, stop or checkpoint.
Fix: the Invoke handler's durable-execution branch and its response serializer, plus the durable-execution state machine.
Traces: lambda.durable-execution-name-conflict-rejected, lambda.durable-execution-server-managed-append-only-history, lambda.get-durable-execution-state-invalid-checkpoint-token-rejected, lambda.stop-durable-execution-already-terminal-is-a-no-op.

Cause 4 of 8 — Invoke's and the control plane's refusal messages are reworded.
AWS: Smithy: Invoke models RequestTooLargeException (413) and InvalidRequestContentException (400); the messages are fixed text with the request's own values substituted. AWS words them: "<n> byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)"; "Could not parse request body into json: Could not parse payload into json: <parser detail>"; "The derived qualifier from the function name does not match the specified qualifier."; "Function not found: <unqualified function ARN>"; "Layer version <full layer version ARN> does not exist."; and for the environment budget "Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. String measured: <...>".
Emulator: returns instead "Request must be smaller than 6291456 bytes for the InvokeFunction operation"; "Could not parse request body into json" alone; "The function name '<name>:<v>' qualifies version '<v>', which does not match the Qualifier parameter '<q>'"; "Function not found: <ARN>:$LATEST" (a qualifier AWS does not append); "Layer version <name>:<n> does not exist" (bare name, no ARN, no trailing period); and an "Environment.Variables must be smaller than 4096 bytes..." text in place of AWS's. The fixed text of each message is the requirement; the sizes, names, versions, qualifiers and ARNs are values taken from the request and from the emulator's state.
Why: clients and CDK/SAM tooling that match on these messages mis-handle the refusal, and the extra ":$LATEST" makes a "function not found" report name a resource the caller never asked for.
Fix: the message construction in each operation's validation (Invoke, GetLayerVersion/DeleteLayerVersion, CreateFunction/UpdateFunctionConfiguration environment validation).
Traces: lambda.invoke-payload-too-large, lambda.dry-run-does-not-invoke-handler, lambda.invoke-qualifier-conflicting-with-name-qualifier-rejected, lambda.event-invoke-failure-not-surfaced, lambda.delete-layer-version, lambda.env-var-size-and-reserved-key-rejected.

Cause 5 of 8 — refusals that never happen on the invoke path.
AWS: Invoke on a function ARN whose region is not the endpoint's region is refused; a function whose code was signed under a CodeSigningConfig with UntrustedArtifactOnDeployment=Enforce must refuse an unsigned package at CreateFunction/UpdateFunctionCode; UpdateFunctionConfiguration while LastUpdateStatus=InProgress is refused with ResourceConflictException.
Emulator: the cross-region Invoke returns 200 with the handler's payload; the unsigned package is accepted under Enforce; the in-flight update conflict never materialises because LastUpdateStatus is never observed as InProgress.
Why: a caller can invoke across regions and deploy unsigned code that real AWS refuses, so code-signing and region isolation give no protection.
Fix: the ARN/region check in the Invoke handler, the code-signing gate in CreateFunction/UpdateFunctionCode, and the function-state machine that drives LastUpdateStatus.
Traces: lambda.invoke-refuses-a-function-arn-from-another-region, lambda.code-signing-enforce-rejects-unsigned-package-rejected, lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code, lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected, lambda.concurrent-function-update-conflict.

Cause 6 of 8 — GetFunction's Code.ResolvedImageUri is not the pushed manifest digest, and CreateFunction accepts an ImageUri that names no image.
AWS: Smithy: GetFunction returns Code (FunctionCodeLocation); for PackageType=Image, ResolvedImageUri is the repository URI with the `@sha256:<digest>` of the manifest the function is pinned to — the digest PutImage returned. CreateFunction with an ImageUri naming no image is refused, because there is nothing to run.
Emulator: ResolvedImageUri carries a digest unrelated to the pushed manifest, and CreateFunction with an ImageUri naming no image succeeds.
Why: image-based deployments cannot be verified or reproduced, and a function can be created that can never run.
Fix: the image-resolution step in CreateFunction/UpdateFunctionCode and the GetFunction serializer.
Traces: lambda.arm64-image-function-runtime-reports-aarch64, lambda.image-runtime-honors-entrypoint-cmd-workdir-and-merges-env, lambda.image-function-runs-or-refuses-honestly.

Cause 7 of 8 — CreateEventSourceMapping accepts an EventSourceArn naming a resource that does not exist.
AWS: CreateEventSourceMapping for a DocumentDB/MQ/MSK source validates the source and refuses with InvalidParameterValueException when the cluster/broker does not exist.
Emulator: returns 202 with State=Creating and a usable EventSourceMappingArn for an absent `arn:aws:rds:...:cluster:<absent>`.
Why: a mapping is created that can never poll anything, and the caller is told its pipeline is wired.
Fix: the source validation in the CreateEventSourceMapping handler.
Traces: lambda.docdb-esm-without-database-name-rejected-before-any-access-check, lambda.event-source-mapping-concurrent-update-conflict.

Cause 8 of 8 — the deployment package's bytes are not preserved: non-ASCII file names vanish and oversize responses are reported with the wrong error.
AWS: the extracted package preserves file names byte-exactly, including non-ASCII and emoji names; a response over 6 MB is refused as Runtime.ResponseSizeTooLarge with the response-size message, not as a generic exit error.
Emulator: the guest sees an empty extraction ([]) where four non-ASCII names were packaged, and an oversize response comes back as {"errorType":"Runtime.ExitError","errorMessage":"the response is larger than the invocation may return"}.
Why: packages with internationalised file names deploy but cannot be read, and the oversize-response failure is unattributable.
Fix: the deployment-package extraction in the runtime component and the response-size check in the invoke path.
Traces: lambda.package-non-ascii-file-names-preserved-byte-exact, lambda.sync-response-at-the-payload-limit-delivered-intact-one-byte-over-rejected, lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running, lambda.warm-start-reuse, lambda.logging-config-json-format-filters-application-log-level, lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent.

## Evidence
Cause 1: HTTP 502 InvalidRuntimeException "The runtime nodejs22.x is not supported by this emulator, which runs the Python runtimes." (also provided.al2023, java21, java17, java8.al2, dotnet10, ruby3.3/3.4). Expected: the function runs.
Cause 2: 202 accepted, then "completion marker 'marker-...' never appeared in /aws/lambda/<fn> - the async invocation was accepted but the function never actually ran"; "expected >= 1 log line(s) matching '<marker>' in /aws/lambda/<fn> within 45s, found 0"; "Check timed out after 60.0s".
Cause 3: "Invoke did not return a DurableExecutionArn; durable functions unsupported here"; KeyError 'DurableExecutionArn'; DescribeDurableExecution stuck at Status='RUNNING' past 45 s.
Cause 4: got 'Request must be smaller than 6291456 bytes for the InvokeFunction operation', expected '<n> byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)'; got 'Could not parse request body into json'; got "The function name '<n>:1' qualifies version '1', which does not match the Qualifier parameter '$LATEST'", expected 'The derived qualifier from the function name does not match the specified qualifier.'; got 'Function not found: <arn>:$LATEST', expected 'Function not found: <arn>'; got 'Layer version <name>:1 does not exist', expected 'Layer version <layer-version-arn> does not exist.'
Cause 5: cross-region Invoke returned HTTP 200 StatusCode 200; unsigned package under Enforce "SUCCEEDED"; "no padded UpdateFunctionCode read back LastUpdateStatus=InProgress in 3 attempts (last 'Successful')".
Cause 6: ResolvedImageUri '<repo>@sha256:9dba1b...' where PutImage returned '@sha256:23947c8f...'; "CreateFunction with an ImageUri naming no image succeeded".
Cause 7: HTTP 202, State='Creating', EventSourceArn 'arn:aws:rds:us-west-2:...:cluster:<absent>'. Expected: a ClientError.
Cause 8: "expected ['données/résumé.txt', 'emoji/..-launch.txt', ...], the guest saw []"; oversize response -> {"errorType":"Runtime.ExitError","errorMessage":"the response is larger than the invocation may return"}.

## Steps to Reproduce
1. CreateFunction Runtime=nodejs22.x and call Invoke / InvokeWithResponseStream -> 502 InvalidRuntimeException (cause 1).
2. Invoke with InvocationType=Event and poll /aws/lambda/<fn> via FilterLogEvents -> no events (cause 2).
3. Invoke with X-Amz-Durable-Execution-Name and read the X-Amz-Durable-Execution-Arn header (cause 3).
4. Invoke with a 6291457-byte payload; Invoke with a non-JSON body; Invoke 'name:1' with Qualifier='$LATEST'; Invoke an absent function; DeleteLayerVersion for an absent version; CreateFunction with >4 KB of environment variables — compare each message (cause 4).
5. Invoke a function ARN from another region; CreateFunction with an unsigned zip under a CodeSigningConfig set to Enforce; UpdateFunctionConfiguration during an in-flight UpdateFunctionCode (cause 5).
6. PutImage to ECR, CreateFunction PackageType=Image with that tag, GetFunction and compare Code.ResolvedImageUri; CreateFunction with an ImageUri naming no image (cause 6).
7. CreateEventSourceMapping with EventSourceArn naming an absent DocumentDB cluster (cause 7).
8. CreateFunction with a zip containing non-ASCII file names and list them from the handler; return a >6 MB payload (cause 8).
