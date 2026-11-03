Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.create-stack-instances-duplicate-operation-id cloudformation.duplicate-stackset-name-rejected cloudformation.stackset-create-duplicate-name-rejected cloudformation.stack-set-operation-timeout-enforcement cloudformation.stack-set-operation-lifecycle-tracking cloudformation.stackset-not-found-and-not-empty-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.create-stack-instances-duplicate-operation-id|run#14|AssertionError|assert code == "OperationIdAlreadyExistsException", f"expected OperationIdAlreadyExistsException, got {code!r}"
Issue-Key: cloudformation.duplicate-stackset-name-rejected|run#9|AssertionError|assert err.get("Message") == "StackSet already exists.", (
Issue-Key: cloudformation.stackset-create-duplicate-name-rejected|run#13|AssertionError|assert msg == "StackSet already exists.", msg
Issue-Key: cloudformation.stack-set-operation-timeout-enforcement|run#17|AssertionError|assert msg2 == f"Cannot Stop since Operation is in {result['Status']}", msg2
Issue-Key: cloudformation.stack-set-operation-lifecycle-tracking|run#19|AssertionError|assert not any(r["Status"] == "SUCCEEDED" for r in results), results
Issue-Key: cloudformation.stackset-not-found-and-not-empty-errors|run#16|AssertionError|assert "Version" in summary, f"get_template_summary did not return a template summary: {summary!r}"
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation StackSet control plane: duplicate OperationId gets the wrong error class, NameAlreadyExistsException interpolates the stack set name, StopStackSetOperation misreports the operation state, operation results never leave SUCCEEDED, and GetTemplateSummary omits Version (5 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackInstances

## Description
Five net-new behavioral root causes of service `cloudformation`, grounded in six failing behavioral checks. Each cause is independent and must be closed separately. The open issue 20261026T235900Z-cloudformation-events-sqs-stackset-operations-never-settle-and.md owns different replay keys and does not cover any cause below.

=== Cause 1 of 5: a replayed OperationId is refused as OperationInProgress instead of OperationIdAlreadyExists ===
AWS: Smithy models CreateStackInstances.OperationId (ClientRequestToken, @idempotencyToken, @length 1..128, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$) and BOTH `OperationIdAlreadyExistsException` (errorKind client, httpError 409, awsQueryError OperationIdAlreadyExistsException/409) and `OperationInProgressException` (client, 409, OperationInProgressException/409). They are distinct conditions: OperationIdAlreadyExists means THIS OperationId has already been used on this stack set; OperationInProgress means a DIFFERENT operation is still running and the stack set cannot take another.
Emulator: re-sending an OperationId that was already used answers `OperationInProgressException`.
Why: the two errors prescribe opposite client actions. OperationIdAlreadyExists is the idempotency signal — the caller must NOT retry, the work already happened. OperationInProgress is a retry-after signal. A client reading the emulator's answer backs off and retries a request that AWS says is already applied, producing duplicate deploy attempts and false timeouts.
Fix: the duplicate-OperationId branch of the CreateStackInstances handler in the cloudformation service's stack-set ops — distinguish 'this token was seen before' from 'another operation is running' and raise the modelled OperationIdAlreadyExistsException for the former.
Traces: cloudformation.create-stack-instances-duplicate-operation-id.

=== Cause 2 of 5: NameAlreadyExistsException's message interpolates the stack set name ===
AWS: Smithy models CreateStackSet error `NameAlreadyExistsException` (client, httpError 409, awsQueryError NameAlreadyExistsException/409). Message is recorded behavior only and is the fixed sentence `StackSet already exists.` — it interpolates NOTHING, not the StackSetName and not the StackSetId.
Emulator: same status and code, message `StackSet <StackSetName> already exists` — a different template that embeds the requested name and drops the trailing period.
Why: callers that compare Error.Message against AWS (taxonomy suites, CLI-output diffs, log matchers) read a string AWS never emits; because the emulator's variant embeds a per-run generated name it is also not stable across runs.
Fix: the CreateStackSet duplicate-name refusal in the cloudformation service's stack-set ops — emit the fixed `StackSet already exists.`
Traces: cloudformation.duplicate-stackset-name-rejected, cloudformation.stackset-create-duplicate-name-rejected.

=== Cause 3 of 5: StopStackSetOperation reports the operation as STOPPED when it is not ===
AWS: Smithy models StopStackSetOperation with OperationNotFoundException, StackSetNotFoundException and InvalidOperationException; recorded behavior is that an operation in a non-stoppable state is refused naming THAT state, and an operation still running is stopped.
Emulator: answers `StackSet operation <OperationId> is in STOPPED state and cannot be stopped` for an operation the check has not stopped — the state it reports is not the operation's actual state, and the refusal therefore fires where AWS either stops the operation or names a different state.
Why: StackSet timeout/cancellation workflows poll and then stop operations. A client told the operation is already STOPPED stops polling and reports the operation cancelled when it is not, so the real state of the deployment is lost.
Fix: the operation-state bookkeeping behind StopStackSetOperation in the cloudformation service's stack-set ops (the state transition recorded for an operation, and the guard that reads it).
Traces: cloudformation.stack-set-operation-timeout-enforcement.

=== Cause 4 of 5: stack-set operation results report a single SUCCEEDED row and never the sequence AWS reports ===
AWS: recorded behavior only — ListStackSetOperationResults reports one entry per account/region target with the status it actually went through, so a tracked operation's result set matches the operation's progression.
Emulator: returns exactly `[{'Account': '000000000000', 'Region': 'us-west-2', 'Status': 'SUCCEEDED'}]` where the check asserts a different result set, i.e. the per-target result rows are synthesised as an immediately-succeeded singleton rather than recorded from the operation.
Why: a client that tracks a StackSet rollout per target cannot see partial failure, in-progress targets, or more than one target at all; every operation looks like a clean single-target success.
Fix: per-target operation-result recording in the cloudformation service's stack-set operation engine, read by ListStackSetOperationResults.
Traces: cloudformation.stack-set-operation-lifecycle-tracking.

=== Cause 5 of 5: GetTemplateSummary omits most of its output members ===
AWS: Smithy models GetTemplateSummaryOutput with Parameters, Description, Capabilities, CapabilitiesReason, ResourceTypes, `Version`, `Metadata`, `DeclaredTransforms`, `ResourceIdentifierSummaries` and `Warnings`. For a template that declares AWSTemplateFormatVersion, `Version` is present in the response.
Emulator: the 200 response carries only Parameters and ResourceTypes — Version is absent (and so are Metadata, DeclaredTransforms and ResourceIdentifierSummaries; the sibling owned check cloudformation.ec2mr-cfnres-in-progress-stack-omits-unstarted-resource dies on KeyError 'ResourceIdentifierSummaries' from the same gap).
Why: GetTemplateSummary is how the console, the CLI's `deploy`, and import/change-set tooling learn a template's format version, declared transforms and importable resource identifiers. Consumers that read those members get a KeyError or silently treat the template as having no transforms and no importable resources.
Fix: the GetTemplateSummary handler/serializer in the cloudformation service — populate Version from the template's AWSTemplateFormatVersion and emit Metadata, DeclaredTransforms and ResourceIdentifierSummaries.
Traces: cloudformation.stackset-not-found-and-not-empty-errors.

## Evidence
Cause 1 — cloudformation.create-stack-instances-duplicate-operation-id [mismatch]:
  AssertionError: expected OperationIdAlreadyExistsException, got 'OperationInProgressException'
  Smithy: CreateStackInstances errors = InvalidOperationException(400), LimitExceededException(400), OperationIdAlreadyExistsException(409), OperationInProgressException(409), StackSetNotFoundException(404), StaleRequestException(409); OperationId = ClientRequestToken @idempotencyToken @length 1..128 pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$.

Cause 2 — cloudformation.duplicate-stackset-name-rejected [mismatch]:
  AssertionError: expected message 'StackSet already exists.', got 'StackSet sim2real-behav-dupss-fc97c82cae95 already exists'
  cloudformation.stackset-create-duplicate-name-rejected [mismatch]:
  AssertionError: StackSet sim2real-stackset-dup-a60a57de26aa already exists
  (both names are per-run values; AWS's message has no variable part at all)
  Smithy: CreateStackSet errors = CreatedButModifiedException(409), LimitExceededException(400), NameAlreadyExistsException(409, awsQueryError NameAlreadyExistsException/409).

Cause 3 — cloudformation.stack-set-operation-timeout-enforcement [mismatch]:
  AssertionError: StackSet operation 957d5b16-332c-4bf2-867f-465f581d47b5 is in STOPPED state and cannot be stopped
  (the uuid is the per-run OperationId)

Cause 4 — cloudformation.stack-set-operation-lifecycle-tracking [mismatch]:
  AssertionError: [{'Account': '000000000000', 'Region': 'us-west-2', 'Status': 'SUCCEEDED'}]

Cause 5 — cloudformation.stackset-not-found-and-not-empty-errors [mismatch], check line 195:
  assert "Version" in summary
  AssertionError: get_template_summary did not return a template summary: {'Parameters': [], 'ResourceTypes': ['AWS::SNS::Topic'], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Smithy: GetTemplateSummaryOutput members = Parameters, Description, Capabilities, CapabilitiesReason, ResourceTypes, Version, Metadata, DeclaredTransforms, ResourceIdentifierSummaries, Warnings.

## Steps to Reproduce
Cause 1:
1. CreateStackSet(StackSetName=<fresh>, TemplateBody=<single AWS::SNS::Topic>).
2. CreateStackInstances(StackSetName=<that>, Accounts=[<acct>], Regions=['us-west-2'], OperationId='opone') and let it settle.
3. Repeat the identical CreateStackInstances with OperationId='opone'. Observe 409 OperationInProgressException; AWS answers 409 OperationIdAlreadyExistsException.

Cause 2:
1. CreateStackSet twice with the same StackSetName. Observe 409 NameAlreadyExistsException 'StackSet <name> already exists'; AWS: 'StackSet already exists.'

Cause 3:
1. Start a StackSet operation, then StopStackSetOperation(StackSetName=<set>, OperationId=<that operation>). Observe 'StackSet operation <id> is in STOPPED state and cannot be stopped' for an operation that was never stopped.

Cause 4:
1. Run a StackSet operation across targets and poll ListStackSetOperationResults. Observe the single synthesised SUCCEEDED row.

Cause 5:
1. GetTemplateSummary(TemplateBody='{"AWSTemplateFormatVersion":"2010-09-09","Resources":{"T":{"Type":"AWS::SNS::Topic"}}}').
2. Observe a 200 containing only Parameters and ResourceTypes; 'Version' (and Metadata/DeclaredTransforms/ResourceIdentifierSummaries) are absent.
3. Re-replay: cloudformation.create-stack-instances-duplicate-operation-id, cloudformation.duplicate-stackset-name-rejected, cloudformation.stackset-create-duplicate-name-rejected, cloudformation.stack-set-operation-timeout-enforcement, cloudformation.stack-set-operation-lifecycle-tracking, cloudformation.stackset-not-found-and-not-empty-errors.
