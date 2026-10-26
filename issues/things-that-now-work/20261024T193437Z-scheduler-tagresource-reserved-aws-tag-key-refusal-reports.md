Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.tag-resource-reserved-aws-prefix-forbidden scheduler.tag-value-unicode-round-trip scheduler.tag-resource-schedule-arn-forbidden
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.tag-resource-reserved-aws-prefix-forbidden|run#10|AssertionError|assert message == _RESERVED_PREFIX_MESSAGE, (
Issue-Key: scheduler.tag-value-unicode-round-trip|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: scheduler.tag-resource-schedule-arn-forbidden|run#17|AssertionError|assert err.get("Message") == expected_tag_message, (
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Scheduler TagResource: reserved aws: tag-key refusal reports a key-syntax message instead of AWS's system-tag refusal, and the enforced resourceArn pattern is narrower than AWS's

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two distinct root causes in Scheduler's tagging surface. Both must be closed before this issue is submitted. In both, the emulator answers the right status and error code (HTTP 400 ValidationException) and persists nothing; what diverges is the refusal AWS actually reports — in Cause 2 the constraint the emulator ENFORCES and quotes.

Cause 1 of 2: a reserved 'aws:' tag key is refused as a key-syntax problem rather than as a system-tag modification.
AWS: Smithy (com.amazonaws.scheduler#TagResourceInput) models only ResourceArn (required, len 1..1011, pattern) and Tags (TagList, len 0..200) and says nothing about reserved keys, so recorded behavior only: TagResource with a tag key beginning 'aws:' is refused with HTTP 400 ValidationException and the FIXED message 'Caller is an end user and not allowed to modify system tags.' Two independent checks recorded that identical text for two different reserved keys ('aws:reserved' and 'aws:reserved-prefix'), so the message is a constant and does NOT derive from the request — that invariance is the rule.
Emulator: refuses with the right code but a request-derived message that echoes the key and frames the condition as key syntax: 'Tag key aws:reserved is reserved: keys must not begin with aws:.' (same text with the other key substituted).
Why: AWS classifies this as the caller not being allowed to modify system tags (an authorization statement about the reserved namespace, constant for every key), not as a malformed key. A client or operator tool that matches AWS's constant text — or that distinguishes system-tag refusals from syntax complaints — sees a condition AWS never reports, and the reserved-tag negative control in two checks fails.
Fix: the reserved-key branch of the TagResource handler, services/service-com-amazonaws-scheduler-awschronosservice/src/ops/tag_resource.rs lines 158-172 (the message formatted from crate::state::reserved_tag_key / crate::state::RESERVED_TAG_KEY_PREFIX in src/state.rs); apply the same message to the sibling tagging entry points (UntagResource, CreateScheduleGroup Tags) that share that helper.
Traces: scheduler.tag-resource-reserved-aws-prefix-forbidden, scheduler.tag-value-unicode-round-trip (its negative half).

Cause 2 of 2: the resourceArn pattern TagResource enforces (and quotes in its validation message) is narrower than the one AWS enforces.
AWS: tagging accepts only a schedule-GROUP ARN; a schedule ARN is refused with HTTP 400 ValidationException whose message is built from the request and quotes the enforced constraint: "1 validation error detected: Value '<arn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:scheduler:[a-z0-9\\-]+:\\d{12}:schedule-group\\/[0-9a-zA-Z-_.]+" (recorded). The partition segment AWS enforces is 'aws[a-z-]*'.
Emulator: same code and same message form, but the constraint it enforces and quotes is its model's @pattern — Smithy: TagResourceInput$ResourceArn pattern '^arn:aws(-[a-z]+)?:scheduler:[a-z0-9\\-]+:\\d{12}:schedule-group\\/[0-9a-zA-Z-_.]+$' — so the message reads '... pattern: arn:aws(-[a-z]+)?:scheduler:...'.
Why: the two regexes are not equivalent in the partition segment: 'aws[a-z-]*' admits multi-segment partitions ('aws-us-gov', 'aws-iso-b'), while 'aws(-[a-z]+)?' admits at most one '-<letters>' segment. Hypothesis: a schedule-group ARN in such a partition, which real AWS accepts for tagging, is refused by the emulator with this ValidationException; the observed, grounded divergence is that the emulator enforces and reports a different constraint than AWS does for 'resourceArn'.
Fix: the resourceArn pattern constraint applied by the boundary validator for the tagging ops (TagResource, UntagResource, ListTagsForResource) — broaden the enforced/reported partition segment to AWS's, i.e. the TagResourceArn pattern the generated input schema in services/service-com-amazonaws-scheduler-awschronosservice/src/ops/types.rs feeds the validator.
Traces: scheduler.tag-resource-schedule-arn-forbidden.

## Evidence
Cause 1 — TagResource(ResourceArn=<schedule-group arn>, Tags=[{'Key': 'aws:reserved', 'Value': 'x'}]): code ValidationException matches and the tag is correctly not persisted; expected message 'Caller is an end user and not allowed to modify system tags.', actual 'Tag key aws:reserved is reserved: keys must not begin with aws:.' (AssertionError at check_scheduler_tag_resource_reserved_aws_prefix_forbidden.py line 88). Same expected text recorded for key 'aws:reserved-prefix', actual 'Tag key aws:reserved-prefix is reserved: keys must not begin with aws:.' (check_scheduler_tag_value_unicode_round_trip.py line 53) — AWS's text is constant across both keys, the emulator's is not.
Cause 2 — TagResource with a SCHEDULE arn (arn:aws:scheduler:us-west-2:<acct>:schedule/<group>/<name>): code ValidationException matches; expected message "1 validation error detected: Value '<arn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z-]*:scheduler:[a-z0-9\\-]+:\\d{12}:schedule-group\\/[0-9a-zA-Z-_.]+", actual the same sentence with the pattern 'arn:aws(-[a-z]+)?:scheduler:[a-z0-9\\-]+:\\d{12}:schedule-group\\/[0-9a-zA-Z-_.]+' (AssertionError at check_scheduler_tag_resource_schedule_arn_forbidden.py line 215).

## Steps to Reproduce
Setup: name = unique; scheduler.create_schedule_group(Name=name) -> ScheduleGroupArn; sanity-tag it with {'ok': '1'} (accepted).
1 (Cause 1): scheduler.tag_resource(ResourceArn=<group arn>, Tags=[{'Key': 'aws:reserved', 'Value': 'x'}]) -> ValidationException with 'Tag key aws:reserved is reserved: keys must not begin with aws:.' instead of 'Caller is an end user and not allowed to modify system tags.'; repeat with Key='aws:reserved-prefix' to see the emulator's message vary where AWS's is constant.
2 (Cause 2): create a schedule in that group, then scheduler.tag_resource(ResourceArn=<the SCHEDULE arn>, Tags=[{'Key': 'ok', 'Value': '1'}]) -> the '1 validation error detected ... at 'resourceArn' ...' message quotes pattern 'arn:aws(-[a-z]+)?:scheduler:...' instead of AWS's 'arn:aws[a-z-]*:scheduler:...'; the same narrower regex is what the emulator enforces, so a group ARN in a multi-segment partition (arn:aws-us-gov:scheduler:...:schedule-group/g) is refused where AWS accepts it.
