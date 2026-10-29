Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-location-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-location-lifecycle datasync-s3-location-iam-role-validation datasync-task-and-filter-validation-errors datasync-task-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|DescribeLocationS3|4xx->4xx|$.datasyncErrorCode
Issue-Key: datasync|DescribeLocationS3|4xx->4xx|$.errorCode
Issue-Key: datasync|CreateLocationS3|4xx->4xx|$.message
Issue-Key: datasync|DescribeLocationS3|4xx->4xx|$.__type
Issue-Key: datasync|DescribeLocationS3|4xx->4xx|$.message
Issue-Key: datasync|CreateTask|4xx->4xx|$.datasyncErrorCode
Issue-Key: datasync|CreateTask|4xx->4xx|$.errorCode
Issue-Key: datasync|CreateTask|4xx->4xx|$.message
Issue-Key: datasync|DescribeTask|4xx->4xx|$.datasyncErrorCode
Issue-Key: datasync|DescribeTask|4xx->4xx|$.errorCode
Issue-Key: datasync|DescribeTask|4xx->4xx|$.message
Issue-Key: datasync|ListTasks|4xx->4xx|$.message
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# DataSync refusals omit errorCode/datasyncErrorCode and use emulator-authored message text (7 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLocationS3

## Description
Seven distinct root causes in how the DataSync emulator SHAPES its 400 refusals. In every one of them the emulator already picks the right HTTP status (400), so the divergence is entirely in the error envelope: the two error-classification members DataSync carries alongside `message` are never populated, one refusal is classified under the wrong exception type, and five refusal messages are composed by the emulator instead of using DataSync's own text. Smithy model consulted: services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json — shape InvalidRequestException (errorKind client, httpError 400, members `message`, `errorCode`, `datasyncErrorCode`), and operations CreateLocationS3 / DescribeLocationS3 / CreateTask / DescribeTask / ListTasks, whose only modeled client error is InvalidRequestException(400).

=== Cause 1 of 7: the not-found refusal carries no errorCode / datasyncErrorCode ===
AWS: Smithy declares InvalidRequestException with THREE members — `message`, `errorCode` and `datasyncErrorCode` — and DescribeLocationS3 binds it as its only client error. When the requested location ARN is well formed but names a location the account does not hold, DataSync answers HTTP 400 with __type InvalidRequestException and BOTH classification members set to the same discriminator for that refusal class: errorCode = "LocationNotFound" and datasyncErrorCode = "LocationNotFound". Those strings are fixed text for the class, not request-derived.
Emulator: status 400 and __type are right, but `$.errorCode` and `$.datasyncErrorCode` are absent from the body entirely (detail: "$.datasyncErrorCode: missing (expected 'LocationNotFound', got None)", same for $.errorCode). The emulator serializes only `message`.
Why: DataSync's own discriminator lives in these members, not in the exception name — every modeled client failure of every DataSync operation is InvalidRequestException, so a client that branches on a missing location (create-if-absent, idempotent reconcilers, retry-vs-fail decisions) MUST read errorCode/datasyncErrorCode. Against the emulator that read is None for every refusal, so all refusal classes collapse into one indistinguishable error and no branch can be taken.
Fix: the service's error serializer — InvalidRequestException must emit its `errorCode` and `datasyncErrorCode` members, and each refusal site must supply its class discriminator.
Traces: datasync-s3-location-lifecycle#task#10 (DescribeLocationS3).

=== Cause 2 of 7: a malformed DataSync ARN is classified ValidationException instead of InvalidRequestException ===
AWS: DescribeLocationS3.LocationArn is required and constrained to pattern ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$, and the operation's ONLY modeled client error is InvalidRequestException (400). A location ARN that does not satisfy that pattern is therefore refused as __type InvalidRequestException with errorCode = "InvalidParameter" and the message template "Invalid parameter: DataSync ARN (<locationArn>) must match datasync resource regex." — the fixed text is "Invalid parameter: DataSync ARN (" ... ") must match datasync resource regex." and the single variable slot is the offending ARN taken verbatim from the request member.
Emulator: status 400 but __type is 'ValidationException' — an exception the operation does not model at all — with no errorCode member, and the message is the generic Smithy constraint template "1 validation error detected: Value '<arn>' at 'locationArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws|...):datasync:...$", which leaks the model's raw regex to the caller.
Why: a client catching the SDK's modeled InvalidRequestException does not catch ValidationException — the exception is surfaced as an unmodeled/unknown error, so ARN-shape handling that works against AWS throws through against the emulator; and the raw-regex message is not text any AWS client has ever seen.
Fix: the DescribeLocationS3 handler's input validation (and the shared ARN-shape validator behind it) must raise InvalidRequestException with errorCode=InvalidParameter and DataSync's ARN message template, rather than falling through to the generic protocol-level constraint reporter.
Traces: datasync-s3-location-lifecycle#task#11 (DescribeLocationS3).

=== Cause 3 of 7: the missing-S3Config refusal is reported with the generic null-member template ===
AWS: CreateLocationS3 models S3Config as @required (target S3Config, whose BucketAccessRoleArn is itself required). When the member is omitted, DataSync does not emit a generic constraint report — it answers HTTP 400 InvalidRequestException with the fixed message "Must specify S3Config". The whole message is fixed text for this refusal class; it has no variable slot.
Emulator: status 400, but the message is "1 validation error detected: Value null at 's3Config' failed to satisfy constraint: Member must not be null" — the emulator's generic required-member template, with a lowercased member name AWS never uses here.
Why: the message is the only human-readable signal the operator sees, and tests/log-scrapers that assert on DataSync's required-member wording fail against the emulator; the emulator also implies the member is spelled 's3Config' on the wire when the modeled name is S3Config.
Fix: CreateLocationS3's input validation — required-member refusals for this operation must emit DataSync's own text instead of delegating to the generic protocol constraint reporter.
Traces: datasync-s3-location-iam-role-validation#task#10 (CreateLocationS3).

=== Cause 4 of 7: not-found messages name the request member and echo the full ARN instead of the bare resource id ===
AWS: CreateTask takes SourceLocationArn/DestinationLocationArn as full location ARNs, but when one of them names a location that does not exist the refusal (400 InvalidRequestException, errorCode and datasyncErrorCode both "LocationNotFound") identifies the resource by its BARE resource id only: fixed text "Location " + <loc-xxxxxxxxxxxxxxxxx> + " is not found." The variable slot is the resource-id suffix of the ARN from the request — not the ARN, and the message never mentions which input member carried it.
Emulator: the message is "SourceLocationArn arn:aws:datasync:<region>:<account>:location/loc-... is not found." — it prefixes the request member name and substitutes the whole ARN, and both classification members are missing (Cause 1's defect on this path).
Why: a client that parses the failing resource id out of the message (the only place DataSync reports WHICH location was missing) gets an ARN-shaped string plus an unexpected member-name prefix, so the parse fails or yields the wrong token; and because the member name is included, the same logical refusal produces two different texts depending on which side of the task was bad.
Fix: the CreateTask handler's location-resolution refusal path — render DataSync's not-found template from the resource id, and populate errorCode/datasyncErrorCode=LocationNotFound.
Traces: datasync-task-and-filter-validation-errors#task#8 (CreateTask).

=== Cause 5 of 7: the task not-found refusal has the same ARN-vs-id defect and the wrong discriminator is absent ===
AWS: DescribeTask.TaskArn is a full task ARN (pattern ...:task/task-[0-9a-f]{17}), but an unknown task is refused 400 InvalidRequestException with errorCode = datasyncErrorCode = "SyncSetNotFound" (DataSync's internal name for a task) and the message template "Task " + <task-xxxxxxxxxxxxxxxxx> + " is not found." — fixed text around a single variable slot holding the BARE task resource id from the request.
Emulator: the message interpolates the full ARN — "Task arn:aws:datasync:<region>:<account>:task/task-... is not found." — and neither errorCode nor datasyncErrorCode is serialized, so the distinctive SyncSetNotFound discriminator (which differs from the resource name the caller used) is unobservable.
Why: SyncSetNotFound is the string clients key task-missing handling on and it is not derivable from anything else in the response; combined with the ARN-shaped message slot, neither the machine-readable nor the human-readable identification of the missing task matches AWS.
Fix: the DescribeTask handler's not-found path plus the error serializer from Cause 1.
Traces: datasync-task-and-filter-validation-errors#task#12 (DescribeTask) — the same cluster also covers the equivalent step in datasync-task-lifecycle-and-tags.

=== Cause 6 of 7: the list-length constraint message renders the rejected value as a count instead of the rejected items ===
AWS: CreateTask.Excludes (and Includes) target FilterList with @length max 1. When a longer list is sent, DataSync reports the constraint with the rejected VALUE rendered into the message's value slot: "1 validation error detected: Value '[FilterRule(filterType=SIMPLE_PATTERN, value=/a), FilterRule(filterType=SIMPLE_PATTERN, value=/b)]' at 'excludes' failed to satisfy constraint: Member must have length less than or equal to 1". The fixed text is the constraint template ("1 validation error detected: Value '...' at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>"); the variable parts are the member path, the modeled maximum, and the rendering of the submitted FilterRule structures (each as FilterRule(filterType=<FilterType>, value=<Value>) from the request).
Emulator: the same template but the value slot is summarized as "Value '2 items'" — the submitted elements are discarded and replaced with a count.
Why: the value slot is how a caller sees WHICH filter rules were rejected; replacing it with a cardinality means a client (or a human reading a log) cannot tell the offending entries apart, and any assertion on DataSync's constraint rendering fails. It also diverges from how the emulator must render every other collection constraint.
Fix: the shared constraint-violation message formatter — render the rejected collection's elements (structures included) into the value slot rather than emitting an item count.
Traces: datasync-task-and-filter-validation-errors#task#10 (CreateTask).

=== Cause 7 of 7: the enum constraint message lists the allowed value set in the wrong order ===
AWS: ListTasks.Filters[].Operator targets the closed enum Operator. An unmodeled operator is refused 400 with the enum-set constraint message "1 validation error detected: Value '<supplied>' at 'filters.<n>.member.operator' failed to satisfy constraint: Member must satisfy enum value set: [In, BeginsWith, NotContains, NotEquals, GreaterThanOrEqual, LessThanOrEqual, LessThan, GreaterThan, Contains, Equals]". Recorded behavior only for the ORDER: the Smithy enum declares its values as [Equals, NotEquals, In, LessThanOrEqual, LessThan, GreaterThanOrEqual, GreaterThan, Contains, NotContains, BeginsWith], while the recording shows DataSync emitting the set in a different, stable order, so the listing is not simply the model's declaration order. The fixed text is the template and the value-set listing; the variable parts are the supplied value and the member path (both from the request).
Emulator: identical status, code and template, but the value set is printed in model declaration order — "[Equals, NotEquals, In, LessThanOrEqual, LessThan, GreaterThanOrEqual, GreaterThan, Contains, NotContains, BeginsWith]".
Why: the enum listing is part of the message a client sees and asserts on; a differently ordered set makes exact-match comparison of DataSync's validation output impossible even though the permitted values are the same.
Fix: the enum constraint message formatter for this service — emit the allowed-value listing in the service's own order rather than the model's declaration order.
Traces: datasync-task-and-filter-validation-errors#task#14 (ListTasks).

Related open issue, which does NOT cover any of the above: 20261023T110503Z-datasync-createtask-createlocations3-cross-member-and-access-validation.md is a BEHAVIORAL issue about missing cross-member/access validation and the task-execution stub; it says nothing about the error envelope, the classification members or the message templates reported here.

## Evidence
Replay of simparity-wire-aws-v1 against the booted emulator (emulator_up=true, /__admin/health 200). datasync score 47.5% — 58/122 steps passed, 56 mismatched. All seven clusters below are verdict=mismatch with expected_status 400 and actual_status 400, i.e. correct status, wrong envelope.

1) Cause 1 — datasync-s3-location-lifecycle#task#10, DescribeLocationS3:
   $.datasyncErrorCode: missing (expected 'LocationNotFound', got None)
   $.errorCode: missing (expected 'LocationNotFound', got None)

2) Cause 2 — datasync-s3-location-lifecycle#task#11, DescribeLocationS3:
   $.__type: value_mismatch (expected 'InvalidRequestException', got 'ValidationException')
   $.errorCode: missing (expected 'InvalidParameter', got None)
   $.message: expected "Invalid parameter: DataSync ARN (arn:aws:datasync:us-west-2:000000000000:location/loc-zzz) must match datasync resource regex.", got "1 validation error detected: Value 'arn:aws:datasync:us-west-2:000000000000:location/loc-zzz' at 'locationArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:(aws|aws-cn|aws-us-gov|aws-eusc|aws-iso|aws-iso-b):datasync:[a-z\-0-9]+:[0-9]{12}:location/loc-[0-9a-z]{17}$"

3) Cause 3 — datasync-s3-location-iam-role-validation#task#10 (cluster also covers task#11), CreateLocationS3:
   $.message: expected 'Must specify S3Config', got "1 validation error detected: Value null at 's3Config' failed to satisfy constraint: Member must not be null"

4) Cause 4 — datasync-task-and-filter-validation-errors#task#8, CreateTask:
   $.datasyncErrorCode / $.errorCode: missing (expected 'LocationNotFound')
   $.message: expected 'Location loc-0abcdef1234567890 is not found.', got 'SourceLocationArn arn:aws:datasync:us-west-2:000000000000:location/loc-0abcdef1234567890 is not found.'

5) Cause 5 — datasync-task-and-filter-validation-errors#task#12 (cluster also covers datasync-task-lifecycle-and-tags#task#16), DescribeTask:
   $.datasyncErrorCode / $.errorCode: missing (expected 'SyncSetNotFound')
   $.message: expected 'Task task-0abcdef1234567890 is not found.', got 'Task arn:aws:datasync:us-west-2:000000000000:task/task-0abcdef1234567890 is not found.'

6) Cause 6 — datasync-task-and-filter-validation-errors#task#10, CreateTask:
   $.message: expected "1 validation error detected: Value '[FilterRule(filterType=SIMPLE_PATTERN, value=/a), FilterRule(filterType=SIMPLE_PATTERN, value=/b)]' at 'excludes' failed to satisfy constraint: Member must have length less than or equal to 1", got "... Value '2 items' at 'excludes' ..."

7) Cause 7 — datasync-task-and-filter-validation-errors#task#14, ListTasks:
   $.message: expected enum value set "[In, BeginsWith, NotContains, NotEquals, GreaterThanOrEqual, LessThanOrEqual, LessThan, GreaterThan, Contains, Equals]", got "[Equals, NotEquals, In, LessThanOrEqual, LessThan, GreaterThanOrEqual, GreaterThan, Contains, NotContains, BeginsWith]"

Model references: InvalidRequestException {message, errorCode, datasyncErrorCode}, client, httpError 400; CreateLocationS3.S3Config @required; DescribeLocationS3.LocationArn @required + location ARN pattern; CreateTask.Excludes/Includes FilterList @length(max=1); Operator enum declaration order [Equals, NotEquals, In, LessThanOrEqual, LessThan, GreaterThanOrEqual, GreaterThan, Contains, NotContains, BeginsWith].

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 datasync fixtures; each cause is one step of a recorded fixture.

Cause 1: CreateLocationS3 for an S3 bucket, DeleteLocation it (or use any well-formed location ARN the account does not hold), then DescribeLocationS3 on it. Observe 400 with no errorCode/datasyncErrorCode members. Replay: datasync-s3-location-lifecycle#task#10

Cause 2: DescribeLocationS3 with LocationArn='arn:aws:datasync:<region>:<account>:location/loc-zzz' (fails the modeled pattern). Observe __type ValidationException and the raw-regex constraint message. Replay: datasync-s3-location-lifecycle#task#11

Cause 3: CreateLocationS3 with S3BucketArn set and S3Config omitted entirely. Observe the generic "Value null at 's3Config'" template instead of 'Must specify S3Config'. Replay: datasync-s3-location-iam-role-validation#task#10

Cause 4: CreateTask with a syntactically valid SourceLocationArn naming a nonexistent location (loc-0abcdef1234567890) and a real destination location. Observe the member-name prefix and full ARN in the message, and the missing codes. Replay: datasync-task-and-filter-validation-errors#task#8

Cause 5: DescribeTask with a syntactically valid TaskArn for a nonexistent task (task-0abcdef1234567890). Observe the full ARN in the message and no SyncSetNotFound codes. Replay: datasync-task-and-filter-validation-errors#task#12

Cause 6: CreateTask with Excludes=[{FilterType:SIMPLE_PATTERN,Value:'/a'},{FilterType:SIMPLE_PATTERN,Value:'/b'}] (length 2 > max 1). Observe the value slot rendered as '2 items'. Replay: datasync-task-and-filter-validation-errors#task#10

Cause 7: ListTasks with Filters=[{Name:<valid>,Values:[...],Operator:'EQ'}]. Observe the enum value set printed in model declaration order. Replay: datasync-task-and-filter-validation-errors#task#14
