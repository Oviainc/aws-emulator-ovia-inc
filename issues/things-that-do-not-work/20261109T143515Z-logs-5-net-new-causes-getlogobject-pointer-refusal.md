Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.get-log-object-download-url-ttl logs.get-log-object-retrieves-event-payload logs.get-log-object-returns-raw-payload logs.get-log-object-unauthorized-rejected logs.limit-exceeded-triggers logs.op-contract-putResourcePolicy logs.put-bearer-token-authentication-enforced-server-side logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.get-log-object-download-url-ttl|run#10|AssertionError|assert message_malformed == expected_message, (
Issue-Key: logs.get-log-object-retrieves-event-payload|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.get-log-object-returns-raw-payload|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: logs.get-log-object-unauthorized-rejected|run#12|AssertionError|assert positive_message == expected_message, (
Issue-Key: logs.limit-exceeded-triggers|run#13|AssertionError|assert code_msg == "Resource limit exceeded.", (
Issue-Key: logs.op-contract-putResourcePolicy|run#14|AssertionError|assert message == (
Issue-Key: logs.put-bearer-token-authentication-enforced-server-side|run#21|AssertionError|assert status == 403, f"expected 403 before auth is enabled, got {status}"
Issue-Key: logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error|_poll_until#24|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# logs: 5 net-new causes — GetLogObject pointer refusal, resource-policy limit and PutResourcePolicy parse messages are emulator-worded, bearer-token auth answers 404 not 403, and EMFParsingErrors is never published

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLogObject

## Description
Cause 1 of 5: GetLogObject's invalid-pointer refusal is not AWS's message.
AWS: GetLogObject with a logObjectPointer that is not a valid pointer is refused with the fixed message 'Invalid log object pointer format.' (the recording shows that exact sentence for every malformed pointer, across the download-URL TTL, raw-payload, event-payload and unauthorized checks).
Emulator: the message is 'The specified logObjectPointer is not valid.' — emulator-authored wording for the same refusal class; there are no variable parts in this message.
Why: a client matching on the refusal text (or presenting it) sees a sentence AWS never sends, and all four GetLogObject scenarios stop at the refusal, so the payload/TTL/authorization behaviour behind it is unreachable.
Fix: the GetLogObject handler's pointer validation message in the CloudWatch Logs crate.
Traces: logs.get-log-object-download-url-ttl, logs.get-log-object-retrieves-event-payload, logs.get-log-object-returns-raw-payload, logs.get-log-object-unauthorized-rejected.

Cause 2 of 5: the resource-policy limit refusal is not AWS's message.
AWS: exceeding the account's resource-policy quota is refused with LimitExceededException whose message is the fixed sentence 'Resource limit exceeded.'
Emulator: it answers 'an account can have a maximum of 10 resource policies without a resourceArn' — an emulator-authored sentence that also leaks an internal rule.
Why: a client matching AWS's limit message does not recognise the refusal.
Fix: the limit-exceeded message in the PutResourcePolicy handler of the logs crate.
Traces: logs.limit-exceeded-triggers.

Cause 3 of 5: PutResourcePolicy's malformed-document refusal is not AWS's message.
AWS: a policyDocument that is not valid JSON/IAM grammar is refused with InvalidParameterException and the fixed message 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.'
Emulator: it answers 'policyDocument must be a JSON policy document (a JSON object)'.
Why: a client cannot tell a grammar error from another parameter error by message, and tooling keyed to AWS's sentence breaks.
Fix: the policy-document validation message in the PutResourcePolicy handler.
Traces: logs.op-contract-putResourcePolicy.

Cause 4 of 5: a bearer-token-protected endpoint answers 404 where AWS answers 403.
AWS: before bearer-token authentication is enabled on the log group the endpoint refuses an unauthenticated request with HTTP 403; after it is enabled a valid token is accepted.
Emulator: the pre-enable request is answered HTTP 404 — 'expected 403 before auth is enabled, got 404' — i.e. the route does not exist rather than refusing authorization.
Why: a client cannot distinguish 'not authorized' from 'no such resource', and the whole bearer-token flow cannot be validated.
Fix: the bearer-token-authenticated ingestion route and its authorization status in the logs crate (PutBearerTokenAuthentication path).
Traces: logs.put-bearer-token-authentication-enforced-server-side.

Cause 5 of 5: a malformed EMF event publishes no AWS/Logs EMFParsingErrors datapoint.
AWS: ingesting a malformed embedded-metric-format event increments AWS/Logs EMFParsingErrors with the LogGroupName dimension (Sum >= 1), which is the only signal distinguishing 'parsed and refused' from 'never looked at'.
Emulator: the metric never appears — 'AWS/Logs EMFParsingErrors{LogGroupName=<group>} reaches Sum >= 1 after a malformed EMF event ... not met within 420.0s. Last result: []'.
Why: a client cannot detect that its EMF payloads are being dropped; metric extraction silently fails.
Fix: EMF parsing on ingest and the AWS/Logs EMFParsingErrors publisher in the logs crate's PutLogEvents path.
Traces: logs.malformed-embedded-metric-format-event-counts-an-emf-parsing-error.

## Evidence
Cause 1 — 'AssertionError: expected message 'Invalid log object pointer format.', got 'The specified logObjectPointer is not valid.'' (four checks).
Cause 2 — 'AssertionError: expected message 'Resource limit exceeded.', got 'an account can have a maximum of 10 resource policies without a resourceArn''.
Cause 3 — expected 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.'; actual 'unexpected message: 'policyDocument must be a JSON policy document (a JSON object)''.
Cause 4 — 'AssertionError: expected 403 before auth is enabled, got 404'.
Cause 5 — 'AssertionError: AWS/Logs EMFParsingErrors{LogGroupName=sim2real-behav-logs-emferr-<id>} reaches Sum >= 1 after a malformed EMF event ... not met within 420.0s. Last result: []'.

## Steps to Reproduce
Replay the eight logs checks listed in trace_ids: the four get-log-object checks call GetLogObject with a malformed pointer; limit-exceeded-triggers exceeds the resource-policy quota; op-contract-putResourcePolicy posts policyDocument='{not-valid-json:::'; put-bearer-token-authentication-enforced-server-side requests the endpoint before enabling auth; malformed-embedded-metric-format-event-counts-an-emf-parsing-error puts a malformed EMF event and polls AWS/Logs EMFParsingErrors.
