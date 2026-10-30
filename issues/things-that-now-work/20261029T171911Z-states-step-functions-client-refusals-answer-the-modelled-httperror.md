Issue-Source: trace-sweep-divergence
Issue-Trace: states-map-run-inspection-and-lookup-errors
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: states-map-run-inspection-and-lookup-errors states-map-run-monitoring-and-update stepfunctions-map-run-error-paths states-resource-tagging-workflow states-state-machine-versioning-and-aliases stepfunctions-version-alias-blue-green-rollout states-state-machine-alias-lifecycle
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|DescribeMapRun|4xx->4xx|$.message
Issue-Key: states|DescribeMapRun|4xx->4xx|status
Issue-Key: states|UpdateMapRun|4xx->4xx|$.message
Issue-Key: states|UpdateMapRun|4xx->4xx|status
Issue-Key: states|TagResource|4xx->4xx|$.message
Issue-Key: states|TagResource|4xx->4xx|status
Issue-Key: states|DescribeStateMachineAlias|4xx->4xx|$.message
Issue-Key: states|DescribeStateMachineAlias|4xx->4xx|status
Issue-Key: states|CreateStateMachineAlias|4xx->4xx|$.message
Issue-Key: states|CreateStateMachineAlias|4xx->4xx|status
Issue-Key: states|DeleteStateMachineVersion|4xx->4xx|$.message
Issue-Key: states|DeleteStateMachineVersion|4xx->4xx|status
Issue-Key: states|PublishStateMachineVersion|4xx->4xx|status
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Step Functions client refusals answer the modelled @httpError (404/409) instead of this service's HTTP 400 (8 causes)

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeMapRun

## Description
This issue bundles EIGHT distinct net-new wire causes in the Step Functions (`states`) emulator, all sharing one fault: the error serializer maps a client error shape to the HTTP status declared on that shape's `@httpError` trait, while Step Functions is an awsJson1_0 service that answers EVERY modelled client error with HTTP 400. The emulator therefore answers 404 for the `ResourceNotFound` family and 409 for the `ConflictException` family. Several of the same refusals also carry an emulator-authored message. All eight must be closed before submitting; re-replay every fixture#step listed in trace_ids.

=== Cause 1 of 8: DescribeMapRun on a Map Run ARN that names nothing answers 404 instead of 400, with emulator wording ===
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models DescribeMapRun with input {mapRunArn (required, LongArn, @length 1..2000)} and errors InvalidArn (400, defaulted) and ResourceNotFound (@httpError 404, explicitly set). The recorded response contradicts that modelled status: the wire answer is HTTP 400 with error code ResourceNotFound and the message template `Resource not found: '<resourceArn>'` — the fixed text is `Resource not found: '` ... `'` and the only variable part is the ARN taken verbatim from the request's `mapRunArn`.
Emulator: HTTP 404, and the message `could not find the resource <resourceArn>` — `could not find the resource ` is emulator-authored fixed text, and the ARN is interpolated without the surrounding quotes AWS uses.
Why: a client that branches on HTTP status takes the wrong branch — 404 reads as "this endpoint/route does not exist" to proxies, API gateways and SDK retry classifiers, whereas 400 means "a well-formed call about a resource that is absent". Code asserting on the documented message text never matches, so a `ResourceNotFound` handler that parses the message to recover the offending ARN fails.
Fix: the sfn error serializer / status mapping (ResourceNotFound must serialize as 400 for this service rather than taking the modelled @httpError), plus the message construction where DescribeMapRun raises ResourceNotFound.
Traces: states-map-run-inspection-and-lookup-errors#task#5 (also states-map-run-monitoring-and-update#task#5, stepfunctions-map-run-error-paths#task#6).

=== Cause 2 of 8: UpdateMapRun on a Map Run ARN that names nothing answers 404 instead of 400, with the same emulator wording ===
AWS: Smithy models UpdateMapRun with errors InvalidArn (400), ResourceNotFound (@httpError 404) and ValidationException (400). Recorded: HTTP 400, code ResourceNotFound, message `Resource not found: '<mapRunArn from the request>'`.
Emulator: HTTP 404 with `could not find the resource <mapRunArn>`.
Why: same as Cause 1 — the write path is where a caller most needs to distinguish "resource gone" (400/ResourceNotFound, re-resolve and retry) from a transport/routing failure.
Fix: the shared ResourceNotFound status mapping plus UpdateMapRun's not-found message construction.
Traces: states-map-run-inspection-and-lookup-errors#task#6 (also stepfunctions-map-run-error-paths#task#7).

=== Cause 3 of 8: TagResource on a deleted resource answers 404 instead of 400 ===
AWS: Smithy models TagResource with input {resourceArn (required), tags (required)} and errors InvalidArn (400), ResourceNotFound (@httpError 404) and TooManyTags (400). Recorded: HTTP 400, code ResourceNotFound, message `Resource not found: '<resourceArn>'` — here the ARN is the activity ARN from the request.
Emulator: HTTP 404 with `could not find the resource <resourceArn>`.
Why: tagging drivers (Terraform, tag-on-create wrappers) treat 400/ResourceNotFound as "the resource was deleted concurrently, drop it from state" and a 404 as a client/endpoint fault; the wrong status aborts the run instead of reconciling.
Fix: the shared ResourceNotFound status mapping plus the tagging handlers' not-found message.
Traces: states-resource-tagging-workflow#task#12.

=== Cause 4 of 8: DescribeStateMachineAlias on a deleted alias answers 404 instead of 400 ===
AWS: Smithy models DescribeStateMachineAlias with errors InvalidArn (400), ResourceNotFound (@httpError 404) and ValidationException (400). Recorded: HTTP 400, code ResourceNotFound, message `Resource not found: '<stateMachineAliasArn>'`.
Emulator: HTTP 404 with `could not find the resource <stateMachineAliasArn>`.
Why: as Cause 1, on the alias read path a client uses to decide whether to (re)create the alias.
Fix: the shared ResourceNotFound status mapping plus the alias handlers' not-found message. NOTE: open issue 20261025T131505Z-sfn-step-functions-alias-operations-resourcenotfound-returned-as-http.md describes a behavioral check on this same alias status mapping, but it is a BEHAVIORAL issue carrying behavioral check keys only — the replay report marks this wire cluster as owned by nobody, so this wire fixture/step is not gated by it and the two must both be closed.
Traces: states-state-machine-versioning-and-aliases#task#17 (also stepfunctions-version-alias-blue-green-rollout#task#14).

=== Cause 5 of 8: CreateStateMachineAlias colliding with an existing alias of different routing answers 409 instead of 400, with emulator wording ===
AWS: Smithy models CreateStateMachineAlias with errors ConflictException (@httpError 409), InvalidArn, InvalidName, ResourceNotFound (404), ServiceQuotaExceededException (402), StateMachineDeleting and ValidationException. Recorded: the collision answers HTTP 400 with the message `Failed to create alias because an alias with the same name and a different routing configuration already exists.` — fixed text with NO variable part.
Emulator: HTTP 409 with `State Machine Alias Already Exists: '<aliasArn>'`; both the leading text and the interpolated alias ARN are emulator-authored and appear nowhere in AWS's message for this class.
Why: 409 is a status SDK retry policies and proxies treat differently from 400, and a caller matching AWS's documented sentence (the only way to tell this conflict from a quota or validation refusal, since the code is shared) never matches.
Fix: the ConflictException status mapping in the sfn error serializer plus CreateStateMachineAlias's conflict message construction.
Traces: states-state-machine-alias-lifecycle#task#8.

=== Cause 6 of 8: CreateStateMachineAlias naming a version that does not exist answers 404 instead of 400 ===
AWS: Smithy models ResourceNotFound on CreateStateMachineAlias with @httpError 404; the recorded wire status for this service is HTTP 400.
Emulator: HTTP 404 (the body's error code and message matched; only the status diverged).
Why: same client-branching defect as Cause 1, on the alias create path.
Fix: the shared ResourceNotFound status mapping.
Traces: states-state-machine-versioning-and-aliases#task#13 (also stepfunctions-version-alias-blue-green-rollout#task#12).

=== Cause 7 of 8: DeleteStateMachineVersion for a version an alias still references answers 409 instead of 400, with emulator wording ===
AWS: Smithy models DeleteStateMachineVersion with input {stateMachineVersionArn (required)} and errors ConflictException (@httpError 409), InvalidArn (400) and ValidationException (400). Recorded: HTTP 400 with the message template `Version to be deleted must not be referenced by an alias. Current list of aliases referencing this version: [<alias names>]` — the fixed text is everything up to and including `[`, and the variable part is the comma-separated list of the NAMES of the aliases in the emulator's state that route to that version (names, not ARNs).
Emulator: HTTP 409 with `'<versionArn>' is referenced by the state machine alias(es) <aliasArn>, which must be deleted or pointed at another version before it can be deleted` — wholly emulator-authored text, and it lists alias ARNs where AWS lists bare alias names.
Why: a caller cannot parse out which aliases to re-point (the whole point of this message) because the shape of the list differs, and the 409 misclassifies a retryable-after-repoint conflict.
Fix: the ConflictException status mapping plus DeleteStateMachineVersion's conflict message construction (render alias names in a bracketed list).
Traces: states-state-machine-versioning-and-aliases#task#15 (also stepfunctions-version-alias-blue-green-rollout#task#11).

=== Cause 8 of 8: PublishStateMachineVersion conflict answers 409 instead of 400 ===
AWS: Smithy models PublishStateMachineVersion with errors ConflictException (@httpError 409), InvalidArn, ServiceQuotaExceededException (402), StateMachineDeleting, StateMachineDoesNotExist and ValidationException; the recorded wire status for the conflict is HTTP 400.
Emulator: HTTP 409 (body matched; only the status diverged).
Why: as Cause 5 — a publish driver that retries on 409 and fails fast on 400 behaves backwards.
Fix: the ConflictException status mapping in the sfn error serializer.
Traces: stepfunctions-version-alias-blue-green-rollout#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 479/537 steps passed, 58 mismatched). Every line below is verdict=mismatch, service=states.

Cause 1 — states-map-run-inspection-and-lookup-errors#task#5, DescribeMapRun: `status: expected 400, got 404`; `$.message: value_mismatch (expected "Resource not found: '<mapRunArn>'", got 'could not find the resource <mapRunArn>')`. Cluster also covers states-map-run-monitoring-and-update#task#5 and stepfunctions-map-run-error-paths#task#6.
Cause 2 — states-map-run-inspection-and-lookup-errors#task#6, UpdateMapRun: `status: expected 400, got 404`; same message divergence. Also stepfunctions-map-run-error-paths#task#7.
Cause 3 — states-resource-tagging-workflow#task#12, TagResource: `status: expected 400, got 404`; `expected "Resource not found: '<activityArn>'", got 'could not find the resource <activityArn>'`.
Cause 4 — states-state-machine-versioning-and-aliases#task#17, DescribeStateMachineAlias: `status: expected 400, got 404`; `expected "Resource not found: '<aliasArn>'", got 'could not find the resource <aliasArn>'`. Also stepfunctions-version-alias-blue-green-rollout#task#14.
Cause 5 — states-state-machine-alias-lifecycle#task#8, CreateStateMachineAlias: `status: expected 400, got 409`; `expected 'Failed to create alias because an alias with the same name and a different routing configuration already exists.', got "State Machine Alias Already Exists: '<aliasArn>'"`.
Cause 6 — states-state-machine-versioning-and-aliases#task#13, CreateStateMachineAlias: `status: expected 400, got 404` (status only). Also stepfunctions-version-alias-blue-green-rollout#task#12.
Cause 7 — states-state-machine-versioning-and-aliases#task#15, DeleteStateMachineVersion: `status: expected 400, got 409`; `expected 'Version to be deleted must not be referenced by an alias. Current list of aliases referencing this version: [<alias name>]', got "'<versionArn>' is referenced by the state machine alias(es) <aliasArn>, which must be deleted or pointed at another version before it can be deleted"`. Also stepfunctions-version-alias-blue-green-rollout#task#11.
Cause 8 — stepfunctions-version-alias-blue-green-rollout#task#10, PublishStateMachineVersion: `status: expected 400, got 409` (status only).

All ARNs, state-machine/alias/activity names and version numbers quoted above are this run's generated values and are NOT part of the requirement; the requirement is the status, the error code and the fixed text of each message with its variable parts filled from the request and the emulator's own state.

## Steps to Reproduce
Cause 1/2: 1) sfn:CreateStateMachine with a DISTRIBUTED Map definition; 2) call sfn:DescribeMapRun (then sfn:UpdateMapRun{maxConcurrency:1}) with a syntactically valid mapRun ARN that names no Map Run. Expect HTTP 400 / ResourceNotFound / "Resource not found: '<arn>'"; observed HTTP 404 and "could not find the resource <arn>".
Cause 3: 1) sfn:CreateActivity; 2) sfn:DeleteActivity; 3) sfn:TagResource(resourceArn=<that activity arn>, tags=[...]). Expect 400 / ResourceNotFound / "Resource not found: '<arn>'"; observed 404.
Cause 4: 1) create a state machine, publish a version, create an alias; 2) sfn:DeleteStateMachineAlias; 3) sfn:DescribeStateMachineAlias on it. Expect 400; observed 404.
Cause 5: 1) create a state machine and publish two versions; 2) sfn:CreateStateMachineAlias(name=A, routing->v1); 3) sfn:CreateStateMachineAlias(name=A, routing->v2). Expect 400 with "Failed to create alias because an alias with the same name and a different routing configuration already exists."; observed 409 with "State Machine Alias Already Exists: '<arn>'".
Cause 6: sfn:CreateStateMachineAlias whose routingConfiguration names a stateMachineVersionArn that was never published. Expect 400; observed 404.
Cause 7: 1) publish version 1; 2) create an alias routing 100% to version 1; 3) sfn:DeleteStateMachineVersion(version 1). Expect 400 with "Version to be deleted must not be referenced by an alias. Current list of aliases referencing this version: [<alias name>]"; observed 409 with emulator wording naming the alias ARN.
Cause 8: drive sfn:PublishStateMachineVersion into its conflict condition (publish with a stale revisionId / while a concurrent update is in flight). Expect 400; observed 409.
Replay: the fixture#step tokens in Issue-Trace/Issue-Step and trace_ids.
