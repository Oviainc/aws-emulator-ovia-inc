Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.mutating-ops-unknown-trail-fail cloudtrail.op-contract-get-event-selectors cloudtrail.trail-not-found-errors cloudtrail.create-trail-duplicate-name-fails cloudtrail.create-trail-invalid-name-fails cloudtrail.event-selectors-exceeds-limit-rejected cloudtrail.event-selectors-quota-exceeded-rejected cloudtrail.op-contract-put-event-selectors cloudtrail.put-event-selectors-invalid-fails cloudtrail.trail-event-selectors-reject-both-basic-and-advanced cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set cloudtrail.get-insight-selectors-not-enabled-fails cloudtrail.insight-selectors-empty-vs-absent-preserved cloudtrail.list-insights-data-not-enabled-unsupported-operation cloudtrail.op-contract-get-insight-selectors cloudtrail.op-contract-describe-trails cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.mutating-ops-unknown-trail-fail|run#33|AssertionError|assert message == unknown_trail_message, message
Issue-Key: cloudtrail.op-contract-get-event-selectors|run#16|AssertionError|assert message == expected_message, message
Issue-Key: cloudtrail.trail-not-found-errors|run#20|AssertionError|assert got == ("TrailNotFoundException", bogus_arn_message), got
Issue-Key: cloudtrail.create-trail-duplicate-name-fails|run#9|AssertionError|assert msg == f"Trail {name} already exists for customer: {account}", msg
Issue-Key: cloudtrail.create-trail-invalid-name-fails|run#10|AssertionError|assert msg == want_msg, f"{bad_name!r}: {msg!r}"
Issue-Key: cloudtrail.event-selectors-exceeds-limit-rejected|run#12|AssertionError|assert message == "Specify a valid number of selectors (1 to 5) for your trail", message
Issue-Key: cloudtrail.event-selectors-quota-exceeded-rejected|run#9|AssertionError|assert message == "Specify a valid number of selectors (1 to 5) for your trail", message
Issue-Key: cloudtrail.op-contract-put-event-selectors|run#16|AssertionError|assert message == "Specify a valid number of selectors (1 to 5) for your trail", message
Issue-Key: cloudtrail.put-event-selectors-invalid-fails|run#13|AssertionError|assert message == "Specify a valid number of selectors (1 to 5) for your trail", message
Issue-Key: cloudtrail.trail-event-selectors-reject-both-basic-and-advanced|run#8|AssertionError|assert err == (
Issue-Key: cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set|run#21|AssertionError|assert code == "InvalidEventSelectorsException", (
Issue-Key: cloudtrail.get-insight-selectors-not-enabled-fails|run#8|AssertionError|assert message == not_enabled, message
Issue-Key: cloudtrail.insight-selectors-empty-vs-absent-preserved|run#6|AssertionError|assert never_configured_message == not_enabled, never_configured_message
Issue-Key: cloudtrail.list-insights-data-not-enabled-unsupported-operation|run#7|AssertionError|assert baseline_message == (
Issue-Key: cloudtrail.op-contract-get-insight-selectors|run#17|AssertionError|assert message == expected_message, message
Issue-Key: cloudtrail|DescribeTrails|CloudTrailARNInvalidException
Issue-Key: cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn|_assert_error_map#7|AssertionError|assert actual == expected, f"error map mismatch: expected={expected} actual={actual}"
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CloudTrail refusal fidelity: wrong error codes for selectors and event configuration, and refusal messages that do not follow AWS's request-derived templates

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventSelectors

## Description
Nine distinct refusal-fidelity divergences across the trail, event-selector, Insights and event-configuration surfaces. Two of them (Causes 6, 8, 9) are wrong error CODES; the rest are refusal messages whose production rule is known — either AWS's documented fixed template or a template built from the request identifier — so the message is the requirement, not the recorded sample text. There are 9 distinct causes in this issue; all 9 must be closed.

Cause 1 of 9 — TrailNotFoundException does not expand the supplied name to the trail ARN.
AWS: Smithy binds TrailNotFoundException (httpError 404, awsQueryError TrailNotFound) on UpdateTrail/DeleteTrail/StartLogging/GetEventSelectors etc. Recorded behavior: GetTrail echoes the identifier as the caller gave it, while the other trail operations echo the fully-qualified trail ARN, i.e. 'Unknown trail: arn:aws:cloudtrail:<region>:<account>:trail/<name> for the user: <account>'. The rule is request-derived: the identifier in the message is the resolved trail ARN.
Emulator: code and 404 are right, but the message always carries the bare name: 'Unknown trail: sim2real-behav-ct-nonexistent-3735e312d2ab for the user: 000000000000'.
Fix: the trail-lookup error construction shared by the mutating and selector operations must format the resolved ARN, not the raw name.
Traces: cloudtrail.mutating-ops-unknown-trail-fail, cloudtrail.op-contract-get-event-selectors, cloudtrail.trail-not-found-errors.

Cause 2 of 9 — TrailAlreadyExistsException message uses the wrong template.
AWS: Smithy binds CreateTrail error TrailAlreadyExistsException (httpError 400, awsQueryError TrailAlreadyExists). Recorded behavior: the message is 'Trail <name> already exists for customer: <account>' — request-derived (name) plus the caller's account.
Emulator: 'Trail sim2real-behav-ct-trail-33524f143807 already exists in us-west-2' — names the region instead of the account.
Fix: CreateTrail's duplicate-name refusal must build AWS's template.
Traces: cloudtrail.create-trail-duplicate-name-fails.

Cause 3 of 9 — InvalidTrailNameException emits one generic message for every name defect.
AWS: Smithy binds CreateTrail error InvalidTrailNameException (httpError 400, awsQueryError InvalidTrailName). Recorded behavior: the message names the specific rule violated — 'bad name!' -> 'Trail name must end with a letter or number.'; 'ab' -> 'Trail name too short. Minimum allowed length: 3 characters. Specified name length: 2 characters.'; 'my--namespace' -> 'Trail name or ARN cannot have adjacent periods (.), hyphens (-), or underscores (_).' The rule is request-derived: the violated constraint and the offending length are reported.
Emulator: every invalid name gets the single charset message 'Trail name or ARN can only contain ASCII letters (a-z, A-Z), numbers (0-9), periods (.), underscores (_), or dashes (-).'
Fix: CreateTrail's name validator must evaluate the length, terminal-character and adjacent-separator rules separately and report the one that failed.
Traces: cloudtrail.create-trail-invalid-name-fails.

Cause 4 of 9 — PutEventSelectors selector-count refusal message.
AWS: Smithy binds PutEventSelectors error InvalidEventSelectorsException (httpError 400, awsQueryError InvalidEventSelectors). Recorded behavior: more than 5 EventSelectors is refused with 'Specify a valid number of selectors (1 to 5) for your trail' — a validation message naming the member's constraint.
Emulator: code is right; message is 'EventSelectors carries 6 event selectors; a trail can have up to 5'.
Fix: the event-selector count validator's message.
Traces: cloudtrail.event-selectors-exceeds-limit-rejected, cloudtrail.event-selectors-quota-exceeded-rejected, cloudtrail.op-contract-put-event-selectors, cloudtrail.put-event-selectors-invalid-fails.

Cause 5 of 9 — PutEventSelectors basic+advanced mutual-exclusion refusal message.
AWS: Smithy PutEventSelectorsRequest carries both EventSelectors and AdvancedEventSelectors; recorded behavior refuses supplying both with InvalidEventSelectorsException and the message 'You can select events by using either EventSelectors or AdvancedEventSelectors, but not both.'
Emulator: correct code, message 'A PutEventSelectors request carries either EventSelectors or AdvancedEventSelectors, not both'.
Fix: the mutual-exclusion validator's message.
Traces: cloudtrail.trail-event-selectors-reject-both-basic-and-advanced.

Cause 6 of 9 — an unknown ReadWriteType is refused with the wrong error code.
AWS: Smithy binds InvalidEventSelectorsException (400, awsQueryError InvalidEventSelectors) on PutEventSelectors. Recorded behavior: a ReadWriteType value outside the modelled set is refused as InvalidEventSelectorsException. ValidationException is not bound on PutEventSelectors in the model at all.
Emulator: error code 'ValidationException'.
Why: a client catching ct.exceptions.InvalidEventSelectorsException does not catch this, and ValidationException is not an error the operation can raise per the model.
Fix: PutEventSelectors' selector-value validation must classify unknown ReadWriteType as InvalidEventSelectorsException.
Traces: cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set.

Cause 7 of 9 — InsightNotEnabledException message does not follow AWS's template.
AWS: Smithy binds GetInsightSelectors error InsightNotEnabledException (httpError 400, awsQueryError InsightNotEnabled). Recorded behavior: the message is 'Trail <trail arn> does not have Insights enabled. Edit the trail settings to enable Insights, and then try the operation again.' — request-derived (the trail ARN) plus a fixed tail.
Emulator: correct code; message 'Insights event logging is not enabled on <trail arn>: enable it with PutInsightSelectors.'
Fix: the Insights-not-enabled error construction shared by GetInsightSelectors and the Insights data operations.
Traces: cloudtrail.get-insight-selectors-not-enabled-fails, cloudtrail.insight-selectors-empty-vs-absent-preserved, cloudtrail.list-insights-data-not-enabled-unsupported-operation, cloudtrail.op-contract-get-insight-selectors.

Cause 8 of 9 — DescribeTrails rejects a non-ARN entry in trailNameList instead of treating it as a name.
AWS: Smithy DescribeTrailsRequest member trailNameList targets TrailNameList and the operation binds CloudTrailARNInvalidException (httpError 400, awsQueryError CloudTrailARNInvalid) and InvalidTrailNameException (400). Recorded behavior: trailNameList accepts bare trail NAMES as well as ARNs, and entries that match no trail are simply omitted from trailList rather than raising.
Emulator: DescribeTrails raised CloudTrailARNInvalidException 'The ARN arn:aws:cloudtrail:bad is invalid.' and the check aborted before any of its contract assertions ran.
Why: the single most common DescribeTrails call shape (a mix of names and unknown entries) is unusable; a client gets a hard 400 where AWS returns a list.
Fix: DescribeTrails' input handling must accept names, and must not raise for entries that resolve to no trail.
Traces: cloudtrail.op-contract-describe-trails.

Cause 9 of 9 — PutEventConfiguration validates parameter combinations before resolving the event data store.
AWS: Smithy binds PutEventConfiguration errors EventDataStoreNotFoundException (httpError 404, awsQueryError EventDataStoreNotFound) and InvalidParameterCombinationException (400, awsQueryError InvalidParameterCombinationError). Recorded behavior: against an event data store ARN that does not exist, PutEventConfiguration — like EnableFederation, DisableFederation and GetEventConfiguration, which the emulator gets right — answers EventDataStoreNotFoundException with 'The event data store <arn> was not found.'; existence dominates combination validation.
Emulator: PutEventConfiguration answers InvalidParameterCombinationException with 'MaxEventSize must be Large when ContextKeySelectors are ...' for an ARN that names no store, while its three siblings answer EventDataStoreNotFoundException correctly.
Why: a client probing for existence gets a 400 about an unrelated parameter and cannot tell the store is absent; error handling that branches on EventDataStoreNotFoundException is dead.
Fix: PutEventConfiguration's handler must resolve the EventDataStore before running combination validation.
Traces: cloudtrail.event-data-store-federation-and-event-config-reject-unknown-arn.

## Evidence
Cause 1: expected 'Unknown trail: arn:aws:cloudtrail:<region>:<account>:trail/<bogus> for the user: <account>'; got 'Unknown trail: sim2real-behav-ct-nonexistent-3735e312d2ab for the user: 000000000000' (also 'Unknown trail: sim2real-no-such-trail-xyz for the user: 000000000000' and 'Unknown trail: sim2real-behav-ct-nonexistent-15e79401e046 for the user: 000000000000' under code TrailNotFoundException).
Cause 2: expected 'Trail <name> already exists for customer: 000000000000'; got 'Trail sim2real-behav-ct-trail-33524f143807 already exists in us-west-2'.
Cause 3: for Name='bad name!' expected 'Trail name must end with a letter or number.'; got 'Trail name or ARN can only contain ASCII letters (a-z, A-Z), numbers (0-9), periods (.), underscores (_), or dashes (-).'
Cause 4: expected 'Specify a valid number of selectors (1 to 5) for your trail'; got 'EventSelectors carries 6 event selectors; a trail can have up to 5' (4 checks).
Cause 5: expected ('InvalidEventSelectorsException', 'You can select events by using either EventSelectors or AdvancedEventSelectors, but not both.'); got ('InvalidEventSelectorsException', 'A PutEventSelectors request carries either EventSelectors or AdvancedEventSelectors, not both').
Cause 6: "expected InvalidEventSelectorsException for the unknown ReadWriteType, got 'ValidationException'".
Cause 7: expected 'Trail <arn> does not have Insights enabled. Edit the trail settings to enable Insights, and then try the operation again.'; got 'Insights event logging is not enabled on arn:aws:cloudtrail:us-west-2:000000000000:trail/<name>: enable it with PutInsightSelectors.'
Cause 8: botocore.errorfactory.CloudTrailARNInvalidException: An error occurred (CloudTrailARNInvalidException) when calling the DescribeTrails operation: The ARN arn:aws:cloudtrail:bad is invalid.
Cause 9: error map mismatch — expected PutEventConfiguration ('EventDataStoreNotFoundException', 'The event data store arn:aws:cloudtrail:us-west-2:000000000000:eventdatastore/28027b52-... was not found.'); actual ('InvalidParameterCombinationException', 'MaxEventSize must be Large when ContextKeySelectors are ...'), while EnableFederation/DisableFederation/GetEventConfiguration matched.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Replay the checks in trace_ids.
3. Manually: (a) UpdateTrail/DeleteTrail/GetEventSelectors on a nonexistent trail NAME and inspect the TrailNotFoundException message; (b) CreateTrail twice with the same name; (c) CreateTrail with Name='bad name!', 'ab', 'my--namespace'; (d) PutEventSelectors with 6 EventSelectors; (e) PutEventSelectors with both EventSelectors and AdvancedEventSelectors; (f) PutEventSelectors with an out-of-set ReadWriteType; (g) GetInsightSelectors on a trail with no Insights; (h) DescribeTrails(trailNameList=['arn:aws:cloudtrail:bad', '<a real trail name>']); (i) PutEventConfiguration with EventDataStore set to a nonexistent event-data-store ARN.
