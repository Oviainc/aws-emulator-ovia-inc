Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.batch-describe-type-configurations-not-found-error cloudformation.condition-true-dependent-on-false-condition-dependency-skipped cloudformation.get-template-returns-submitted-body-while-review-in-progress cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original cloudformation.stack-create-timeout-in-minutes-rolls-back
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.batch-describe-type-configurations-not-found-error|run#7|AssertionError|assert resp["TypeConfigurations"] == [], (
Issue-Key: cloudformation.condition-true-dependent-on-false-condition-dependency-skipped|expect_error#4|AssertionError|raise AssertionError(f"expected {code} but the call succeeded: {result!r}")
Issue-Key: cloudformation.get-template-returns-submitted-body-while-review-in-progress|run#19|AssertionError|assert by_stack_name["TemplateBody"] == "", (
Issue-Key: cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter|run#23|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original|run#24|AssertionError|assert naming, (
Issue-Key: cloudformation.stack-create-timeout-in-minutes-rolls-back|_poll#7|AssertionError|raise AssertionError(f"{what} not reached within {timeout_s}s; last {last!r}")
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# cloudformation: 6 net-new behavioral causes — BatchDescribeTypeConfigurations invents a configuration, a false-condition dependency is accepted, GetTemplate leaks the unexecuted body, a colliding rename is not rolled back, an export-not-found reason carries a trailing period, and TimeoutInMinutes rollback never settles

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeTypeConfigurations

## Description
Six independent defects in the CloudFormation emulator, each grounded in one failing behavioral check. Each is a separate root cause and must be closed separately.

Cause 1 of 6: BatchDescribeTypeConfigurations resolves a configuration for a type that has none.
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation) — BatchDescribeTypeConfigurations output declares TypeConfigurations, UnprocessedTypeConfigurations and Errors, and models TypeConfigurationNotFoundException (httpError 404, awsQueryError code TypeConfigurationNotFoundException). A type identifier for which SetTypeConfiguration was never called is reported under UnprocessedTypeConfigurations/Errors with that error code — it is NOT returned as a resolved TypeConfigurations entry.
Emulator: the call answers 200 with a fabricated resolved entry, Configuration '{}' and IsDefaultConfiguration false, for a type whose configuration was never set.
Why: a client that reads TypeConfigurations to decide whether an extension is configured concludes it is configured and skips SetTypeConfiguration; the Errors/UnprocessedTypeConfigurations branch of every caller is dead code.
Fix: the BatchDescribeTypeConfigurations handler — it must look the identifier up in type-configuration state and route a miss into UnprocessedTypeConfigurations + Errors with TypeConfigurationNotFoundException instead of defaulting a row.
Traces: cloudformation.batch-describe-type-configurations-not-found-error

Cause 2 of 6: a template whose true Condition depends on a false Condition is accepted instead of refused.
AWS: recorded behavior only — CreateStack refuses the template with 400 ValidationError at submit time; the stack is never created.
Emulator: CreateStack answers 200 and returns a StackId and OperationId for exactly that template.
Why: a template CloudFormation would reject at submit is deployed, so the caller never sees the authoring error and gets a stack whose condition graph was resolved by rules AWS does not use.
Fix: template condition-graph validation in the CreateStack request path, before any stack row is minted.
Traces: cloudformation.condition-true-dependent-on-false-condition-dependency-skipped

Cause 3 of 6: GetTemplate on a REVIEW_IN_PROGRESS stack returns the submitted body.
AWS: Smithy — GetTemplate.TemplateBody is @length min 1 and optional; for a stack in REVIEW_IN_PROGRESS no template has been executed yet, so AWS returns an empty body (no TemplateBody) and reports the stages that are available.
Emulator: GetTemplate returns the full submitted template (AWSTemplateFormatVersion, Resources, Outputs) for a stack still in REVIEW_IN_PROGRESS.
Why: a caller using GetTemplate to tell "has this change set been executed yet" apart from "this stack has a template" cannot — it reads a body for a stack that has none, so an unexecuted change set is indistinguishable from a deployed one.
Fix: the GetTemplate handler's stage selection — the Original/Processed stage must resolve against the last EXECUTED template, not the last SUBMITTED one.
Traces: cloudformation.get-template-returns-submitted-body-while-review-in-progress

Cause 4 of 6: an update renaming a metric filter onto an already-used name reaches UPDATE_COMPLETE.
AWS: recorded behavior only — the rename collides with the existing filter, the resource update fails and the stack settles UPDATE_ROLLBACK_COMPLETE, leaving the unrelated filter intact.
Emulator: the update settles UPDATE_COMPLETE; the collision is never detected.
Why: a client that relies on CloudFormation to refuse a colliding rename sees a successful update while one of the two filters has been silently clobbered — the rollback path is never exercised and the surviving state differs from AWS's.
Fix: the AWS::Logs::MetricFilter resource handler's update path (name-uniqueness check against existing filters in the owning log group) and the stack-update rollback that failure must drive.
Traces: cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter

Cause 5 of 6: the export-not-found stack-event reason carries a trailing period AWS does not send.
AWS: recorded behavior only — the failure reason is exactly `No export named <export-name> found`, where `No export named ` and ` found` are fixed text and <export-name> is the Fn::ImportValue argument from the template. There is no terminating period.
Emulator: the event reason is `No export named <export-name> found.` — the same fixed text with an added '.' — on both the UPDATE_ROLLBACK_IN_PROGRESS and UPDATE_ROLLBACK_COMPLETE events.
Why: a client that matches the documented reason exactly (log scrapers, CI gates, retry classifiers keyed on stack-event reasons) does not recognise the failure and treats an export-resolution error as an unknown rollback cause.
Fix: the Fn::ImportValue resolution error's message construction in the stack-event writer.
Traces: cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original

Cause 6 of 6: a stack that trips TimeoutInMinutes never leaves ROLLBACK_IN_PROGRESS.
AWS: recorded behavior only — when the create exceeds TimeoutInMinutes, CloudFormation rolls back and the stack reaches the terminal status ROLLBACK_COMPLETE (or ROLLBACK_FAILED); it always settles.
Emulator: the stack is still ROLLBACK_IN_PROGRESS after 600s of polling DescribeStacks; no terminal status is ever reached.
Why: every client that waits on a terminal stack status (the CLI waiters, cdk, any poll loop) hangs forever on a timed-out stack instead of observing the rollback, and the stack can never be deleted or retried.
Fix: the timeout-driven rollback path in the stack state machine — the rollback it starts must run to a terminal status.
Traces: cloudformation.stack-create-timeout-in-minutes-rolls-back

## Evidence
[mismatch] cloudformation.batch-describe-type-configurations-not-found-error — AssertionError: expected nothing resolved, got [{'Configuration': '{}', 'TypeArn': 'arn:aws:cloudformation:<region>:<account>:type/resource/<TypeName>', 'TypeName': '<Namespace>::<Service>::<Resource>', 'IsDefaultConfiguration': False}]

[mismatch] cloudformation.condition-true-dependent-on-false-condition-dependency-skipped — AssertionError: expected ValidationError but the call succeeded: {'StackId': 'arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>', 'OperationId': '<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

[mismatch] cloudformation.get-template-returns-submitted-body-while-review-in-progress — AssertionError: GetTemplate(StackName=...) on a REVIEW_IN_PROGRESS stack (no executed template yet) returned OrderedDict({'AWSTemplateFormatVersion': '2010-09-09', 'Resources': {...}, 'Outputs': {...}}), expected an empty body

[mismatch] cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter — AssertionError: rename to the colliding name ended 'UPDATE_COMPLETE', expected UPDATE_ROLLBACK_COMPLETE

[mismatch] cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original — AssertionError: expected a stack event whose reason is exactly 'No export named <export-name> found', got [(<stack>, 'UPDATE_ROLLBACK_COMPLETE', 'No export named <export-name> found.'), (<stack>, 'UPDATE_ROLLBACK_IN_PROGRESS', 'No export named <export-name> found.'), ...]

[mismatch] cloudformation.stack-create-timeout-in-minutes-rolls-back — AssertionError: stack arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid> settled not reached within 600.0s; last 'ROLLBACK_IN_PROGRESS'

## Steps to Reproduce
Replay the behavioral suite and re-run these six checks under the `cloudformation` service:
1. cloudformation.batch-describe-type-configurations-not-found-error — call BatchDescribeTypeConfigurations with a TypeConfigurationIdentifier for a registered private type whose configuration was never set; expect it under UnprocessedTypeConfigurations/Errors with TypeConfigurationNotFoundException, not under TypeConfigurations.
2. cloudformation.condition-true-dependent-on-false-condition-dependency-skipped — CreateStack with a template whose evaluated-true Condition references a false Condition; expect 400 ValidationError.
3. cloudformation.get-template-returns-submitted-body-while-review-in-progress — CreateChangeSet (CREATE) so the stack is REVIEW_IN_PROGRESS, then GetTemplate(StackName=...) without executing; expect an empty body.
4. cloudformation.metricfilter-colliding-rename-rollback-keeps-unrelated-filter — deploy two AWS::Logs::MetricFilter resources, UpdateStack renaming one onto the other's name; expect UPDATE_ROLLBACK_COMPLETE and both original filters intact.
5. cloudformation.redeploy-broken-importvalue-rolls-back-preserving-original — UpdateStack with Fn::ImportValue naming a nonexistent export; read DescribeStackEvents and compare the reason byte-for-byte to `No export named <name> found`.
6. cloudformation.stack-create-timeout-in-minutes-rolls-back — CreateStack with TimeoutInMinutes smaller than the create takes; poll DescribeStacks for a terminal status.
