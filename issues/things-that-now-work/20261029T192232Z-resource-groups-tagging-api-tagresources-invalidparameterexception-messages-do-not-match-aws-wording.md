Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi.tag-resources-malformed-arn-rejected resourcegroupstaggingapi.tag-resources-cross-region-arn-rejected
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi.tag-resources-malformed-arn-rejected|run#12|AssertionError|assert message == expected, (
Issue-Key: resourcegroupstaggingapi.tag-resources-cross-region-arn-rejected|run#16|AssertionError|assert message == expected_message, (
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# TagResources InvalidParameterException messages do not match AWS wording for malformed and cross-region ARNs

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
TagResources correctly refuses both an unparseable ARN and an out-of-region ARN with HTTP 400 / InvalidParameterException and correctly leaves the whole batch unwritten, but the error MESSAGE text it emits is its own invention in both cases rather than AWS's wording. Two distinct message templates are wrong; both must be closed before this issue is submitted.

Related but NOT covering: open issue 20261022T140612Z-resourcegroupstaggingapi-resource-groups-tagging-api-error-contract-diverges-malformed.md. Its Cause 1 is the STATUS/CODE classification of a malformed ARN (then 200, now correctly 400) and quotes AWS's message as an example; it is a wire-corpus issue keyed on status/__type and does not carry these behavioral checks' replay keys. The message templates below are filed here because they are still diverging after the classification was corrected.

Cause 1 of 2: malformed-ARN refusal message (check resource-groups-tagging-api.tag-resources-malformed-arn-rejected).
AWS: a ResourceARNList value that cannot be parsed as an ARN is a request-level validation failure answered with HTTP 400, error code InvalidParameterException, and a message of the form "<value> is not a valid AmazonResourceName (ARN)". Fixed text: " is not a valid AmazonResourceName (ARN)"; the only variable part is the offending ResourceARNList element, echoed verbatim at the START of the message. Smithy (services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/...json): TagResources.ResourceARNList -> ResourceARNListForTagUntag (required, len 1..20); modeled errors InternalServiceException(500), InvalidParameterException(400, client), ThrottledException(400). The model constrains no message text, so the template above is recorded/measured behavior only.
Emulator: answers 400 InvalidParameterException with "ResourceARNList contains a value that is not an Amazon Resource Name: <value>" — a different fixed text, the member name prepended, and the value moved to the end.
Why: the message is the only part of this refusal that identifies WHICH value was bad and in what way. Clients, SDK retry/classification wrappers and user-facing CLIs that match or surface AWS's text (and tests asserting it) see text that does not exist in AWS, so parity tests fail and operators get a message AWS never emits.
Fix: the shared message builder behind TagResources' request-level ARN validation — emit AWS's template with the offending value substituted.
Traces: resource-groups-tagging-api.tag-resources-malformed-arn-rejected. Code sites: src/state.rs:411-413 (not_an_arn_message), called from src/state.rs:392 (addressable_arn) via src/ops/tag_resources.rs:221; the same builder is also used by src/ops/get_resources.rs:246.

Cause 2 of 2: cross-region-ARN refusal message (check resource-groups-tagging-api.tag-resources-cross-region-arn-rejected).
AWS: an ARN whose region field differs from the endpoint's region is refused for the WHOLE request with HTTP 400 / InvalidParameterException and a message of the form "Region in the ARN <arn> does not match with the region in which TagResources API is invoked". Fixed text: "Region in the ARN " ... " does not match with the region in which TagResources API is invoked"; the variable parts are the offending ARN (taken from the request) and the invoked operation's name. Smithy: InvalidParameterException is documented as covering "A provided string parameter is malformed" and partition/region mismatch cases; the message text itself is not modeled, so it is recorded behavior only. The status (400), the code (InvalidParameterException) and the request-scoped atomicity all match already — only the text diverges.
Emulator: answers "ResourceARNList contains an ARN in a Region other than <region>, which this request was made in: <arn>" — different fixed text, names the endpoint region instead of the operation, and does not use AWS's "Region in the ARN ... does not match with the region in which <Op> API is invoked" form.
Why: same as Cause 1 — the text is the contract for a region-mismatch diagnosis. Tooling that distinguishes the region-mismatch refusal from the malformed-ARN refusal by message (both share status 400 and code InvalidParameterException, so the message is the ONLY discriminator) misclassifies it.
Fix: the same request-level ARN validation path in crate::state (the region branch of addressable_arn), used by TagResources' handler.
Traces: resource-groups-tagging-api.tag-resources-cross-region-arn-rejected. Code site: src/state.rs:400-405 (region branch of addressable_arn), reached from src/ops/tag_resources.rs:221.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 (23/32 passed, emulator_up=true, bootstrap_signal=false). Both failures are verdict 'mismatch' on service resource-groups-tagging-api and neither is marked ALREADY FILED or OWNED.

Cause 1 — check resource-groups-tagging-api.tag-resources-malformed-arn-rejected, line 45 of the check:
  Request: TagResources ResourceARNList=[<live sqs queue arn>, "<not-an-arn>"], Tags={<key>:"v"}
  Error code: InvalidParameterException (matches AWS)
  Message expected: '<not-an-arn> is not a valid AmazonResourceName (ARN)'
  Message actual:   'ResourceARNList contains a value that is not an Amazon Resource Name: <not-an-arn>'
  AssertionError: tagging with a malformed ARN was refused with 'ResourceARNList contains a value that is not an Amazon Resource Name: sim2real-behav-not-an-arn', expected 'sim2real-behav-not-an-arn is not a valid AmazonResourceName (ARN)'
  (The recorded value 'sim2real-behav-not-an-arn' is the per-run input filling the template's variable slot, not part of the requirement.)

Cause 2 — check resource-groups-tagging-api.tag-resources-cross-region-arn-rejected, line 60 of the check:
  Request: TagResources ResourceARNList=[<live same-region sqs queue arn>, arn:aws:sqs:<foreign-region>:<account>:<name>], Tags={<key>:"v"}
  Error code: InvalidParameterException (matches AWS)
  Message expected: 'Region in the ARN <foreign arn> does not match with the region in which TagResources API is invoked'
  Message actual:   'ResourceARNList contains an ARN in a Region other than <endpoint region>, which this request was made in: <foreign arn>'
  AssertionError: the cross-region refusal message was 'ResourceARNList contains an ARN in a Region other than us-west-2, which this request was made in: arn:aws:sqs:us-east-1:000000000000:sim2real-behav-far-e71a51230719', expected 'Region in the ARN arn:aws:sqs:us-east-1:000000000000:sim2real-behav-far-e71a51230719 does not match with the region in which TagResources API is invoked'
  (Region names, account id and generated queue name are per-run values filling the template.)

Both checks' subsequent assertions (effect-absent read-back of the index, and the positive control that the same batch succeeds once the bad ARN is dropped) were never reached, so atomicity is not itself implicated by this issue.

Model evidence: TagResources input ResourceARNList (required, ResourceARNListForTagUntag len 1..20, member ResourceARN len 1..1011) and Tags (required, TagMap len 1..50); errors InternalServiceException(500, server), InvalidParameterException(400, client), ThrottledException(400, client). No message text is modeled for InvalidParameterException.

Source evidence:
  src/state.rs:390-407 addressable_arn — not_an_arn_message for an unparseable value, and a hand-written "ResourceARNList contains an ARN in a Region other than {region}, which this request was made in: {value}" for a region mismatch.
  src/state.rs:411-413 not_an_arn_message — "ResourceARNList contains a value that is not an Amazon Resource Name: {value}".
  src/ops/tag_resources.rs:218-222 — the per-ARN addressable_arn loop that lifts both strings into InvalidParameterException.

## Steps to Reproduce
Boot the emulator from this checkout and re-replay the two behavioral checks:

Cause 1 — resource-groups-tagging-api.tag-resources-malformed-arn-rejected:
1. Create an SQS queue and read its QueueArn.
2. POST TagResources (X-Amz-Target ResourceGroupsTaggingAPI_20170126.TagResources) with body {"ResourceARNList":["<queue arn>","not-an-arn"],"Tags":{"k":"v"}}.
3. Expect HTTP 400, __type InvalidParameterException, Message "not-an-arn is not a valid AmazonResourceName (ARN)". Observe HTTP 400 / InvalidParameterException with Message "ResourceARNList contains a value that is not an Amazon Resource Name: not-an-arn".

Cause 2 — resource-groups-tagging-api.tag-resources-cross-region-arn-rejected:
1. Create an SQS queue in the endpoint's region and read its QueueArn.
2. POST TagResources with body {"ResourceARNList":["<queue arn>","arn:aws:sqs:<some-other-region>:<account>:<any-name>"],"Tags":{"k":"v"}} against the <endpoint-region> endpoint.
3. Expect HTTP 400, __type InvalidParameterException, Message "Region in the ARN arn:aws:sqs:<some-other-region>:<account>:<any-name> does not match with the region in which TagResources API is invoked". Observe the message "ResourceARNList contains an ARN in a Region other than <endpoint-region>, which this request was made in: <that arn>".
