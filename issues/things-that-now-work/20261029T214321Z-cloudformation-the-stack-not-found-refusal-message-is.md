Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.describe-stack-resource-drifts-list cloudformation.op-contract-describeStackEvents cloudformation.op-contract-getStackPolicy cloudformation.op-contract-listChangeSets cloudformation.op-contract-signalResource cloudformation.changeset-update-type-on-new-stack-fails cloudformation.invalid-stack-name-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.describe-stack-resource-drifts-list|run#34|AssertionError|assert drift_message == expected_drift_message, (
Issue-Key: cloudformation.op-contract-describeStackEvents|run#15|AssertionError|assert message == f"Stack [{missing}] does not exist", (
Issue-Key: cloudformation.op-contract-getStackPolicy|run#17|AssertionError|assert message == f"Stack: {missing} does not exist", (
Issue-Key: cloudformation.op-contract-listChangeSets|run#15|AssertionError|assert message == f"Stack [{missing}] does not exist", (
Issue-Key: cloudformation.op-contract-signalResource|run#13|AssertionError|assert message == f"Stack {missing} does not exist", (
Issue-Key: cloudformation.changeset-update-type-on-new-stack-fails|run#15|AssertionError|assert msg == f"Stack [{bad_name}] does not exist", msg
Issue-Key: cloudformation.invalid-stack-name-rejected|run#16|AssertionError|assert describe_error.get("Message") == expected_describe_message, (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation: the stack-not-found refusal message is 'Stack with id <name> does not exist' on five operations, and an invalid StackName is never refused by its pattern (2 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResourceDrifts

## Description
Two net-new behavioral root causes of service `cloudformation`, grounded in seven failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 2: the ValidationError message for an unknown stack is the emulator's own template, not AWS's ===

AWS: Smithy models no dedicated not-found error for DescribeStackEvents, GetStackPolicy, ListChangeSets, SignalResource, DescribeStackResourceDrifts or CreateChangeSet(ChangeSetType=UPDATE) — each takes StackName (StackNameOrId, @length min 1, pattern ^([a-zA-Z][-a-zA-Z0-9]*)|(arn:\b(aws|aws-us-gov|aws-cn)\b:[-a-zA-Z0-9:/._+]*)$) and the refusal arrives as the service-common ValidationError (HTTP 400, Error.Code `ValidationError`). The message is recorded behavior only: AWS answers `Stack [<StackName>] does not exist` — fixed text `Stack [` + `] does not exist`, with the StackName exactly as the caller sent it as the only variable part.

Emulator: same status and code, but the message is `Stack with id <StackName> does not exist` — a different fixed template (`Stack with id ` + ` does not exist`, no brackets) around the same single variable part. The same wrong template is emitted by all six operations, so it is one shared refusal path, not six.

Why: a client that classifies on Error.Message — error-taxonomy suites, CLI output comparison, log scrapers, and branch logic that distinguishes 'unknown stack' from 'malformed request' — reads a string AWS never emits. Because HTTP status and Error.Code are identical for both, Message is the only discriminator a caller has here.

Fix: the shared stack-lookup refusal in the cloudformation service's stack ops/validation layer used by the DescribeStackEvents, GetStackPolicy, ListChangeSets, SignalResource, DescribeStackResourceDrifts and CreateChangeSet handlers — emit `Stack [<name>] does not exist`. The open issue 20261025T015306Z-cloudformation-refusal-taxonomy-diverges-wrong-not-found-validation.md covers which error CLASS is chosen for other refusals and does not name this message template.

Traces: cloudformation.describe-stack-resource-drifts-list (states both strings), cloudformation.op-contract-describeStackEvents, cloudformation.op-contract-getStackPolicy, cloudformation.op-contract-listChangeSets, cloudformation.op-contract-signalResource, cloudformation.changeset-update-type-on-new-stack-fails.

=== Cause 2 of 2: a syntactically invalid StackName is answered 'does not exist' instead of being refused by the member's pattern ===

AWS: Smithy CreateStack.StackName targets `StackName` and carries no @pattern in the model, so the constraint is enforced server-side; recorded behavior is HTTP 400, Error.Code `ValidationError`, Error.Message `1 validation error detected: Value '<StackName>' at 'stackName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*`. Fixed text is the whole `1 validation error detected: Value '` … `' at 'stackName' failed to satisfy constraint: Member must satisfy regular expression pattern: ` + the pattern; the only variable part is the rejected value echoed from the request. The refusal is made on the INPUT, before any stack lookup, so it fires for a name that could never name a stack (underscores, dots).

Emulator: the same request falls through to the stack lookup and is answered ValidationError `Stack with id <StackName> does not exist` — a not-found diagnosis for a request that is malformed.

Why: the caller is told to create the stack when the real fault is the name. Validation ORDER is part of the contract: input-constraint failures must precede existence resolution, otherwise an unconstrained name can be used to probe stack existence and tooling cannot distinguish 'bad name' from 'absent stack'.

Fix: StackName constraint validation in the cloudformation service's CreateStack (and the shared StackName input validator) — apply the pattern before the stack lookup and emit the constraint message above.

Traces: cloudformation.invalid-stack-name-rejected.

## Evidence
Cause 1 — cloudformation.describe-stack-resource-drifts-list [mismatch]:
  AssertionError: expected 'Stack [sim2real-listdrifts-e0d93aee6761] does not exist', got 'Stack with id sim2real-listdrifts-e0d93aee6761 does not exist'
  (the stack name is a per-run generated value filling the template's one variable slot)
  Same emulator template, asserted as unexpected, on four more operations:
    cloudformation.op-contract-describeStackEvents — AssertionError: unexpected ValidationError message 'Stack with id sim2real-cfn-missing-17670de6d443 does not exist'
    cloudformation.op-contract-getStackPolicy    — ... 'Stack with id sim2real-cfn-missing-5e93668f62e4 does not exist'
    cloudformation.op-contract-listChangeSets    — ... 'Stack with id sim2real-cfn-missing-0e33e63faa04 does not exist'
    cloudformation.op-contract-signalResource    — ... 'Stack with id sim2real-cfn-missing-c2dc6f1e3be6 does not exist'
    cloudformation.changeset-update-type-on-new-stack-fails — AssertionError: Stack with id sim2real-behav-cfn-cs-badtype-d3f56b4f7d90 does not exist
  Smithy: these operations declare no not-found error shape; the refusal is the service-common ValidationError (HTTP 400, awsQueryError code ValidationError). StackName target StackNameOrId @length min 1, pattern ^([a-zA-Z][-a-zA-Z0-9]*)|(arn:\b(aws|aws-us-gov|aws-cn)\b:[-a-zA-Z0-9:/._+]*)$.

Cause 2 — cloudformation.invalid-stack-name-rejected [mismatch]:
  expected: ('ValidationError', "1 validation error detected: Value '<name>' at 'stackName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*")
  actual:   {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'Stack with id sim2real-behav-cfn-bad-50a0bdee4d9d_invalid.name does not exist'}
  request: CreateStack(StackName='sim2real-behav-cfn-bad-50a0bdee4d9d_invalid.name', ...) — the underscore and dot violate the name pattern; the name itself is a per-run value.
  Smithy: CreateStack.StackName target StackName, clientOptional/required, no @pattern in the model (server-side constraint); CreateStack errors = AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException, TokenAlreadyExistsException — so the pattern refusal is the service-common ValidationError.

## Steps to Reproduce
Cause 1:
1. Pick a stack name that does not exist, e.g. 'sim2real-cfn-missing-aaaa'.
2. Call each of DescribeStackEvents, GetStackPolicy, ListChangeSets, SignalResource(LogicalResourceId='X', UniqueId='1', Status='SUCCESS'), DescribeStackResourceDrifts with StackName=<that name>, and CreateChangeSet(StackName=<that name>, ChangeSetType='UPDATE', TemplateBody=<any>).
3. Observe HTTP 400 ValidationError with Message 'Stack with id <name> does not exist'. AWS answers 'Stack [<name>] does not exist'.

Cause 2:
1. CreateStack(StackName='sim2real-bad_name.1', TemplateBody='{"Resources":{"T":{"Type":"AWS::SNS::Topic"}}}').
2. Observe HTTP 400 ValidationError 'Stack with id sim2real-bad_name.1 does not exist'. AWS refuses the INPUT with the stackName pattern-constraint message and never looks the stack up.
3. Control that must keep working: a well-formed but absent name still yields the Cause-1 not-found refusal (with AWS's wording after the fix).
4. Re-replay: cloudformation.describe-stack-resource-drifts-list, cloudformation.op-contract-describeStackEvents, cloudformation.op-contract-getStackPolicy, cloudformation.op-contract-listChangeSets, cloudformation.op-contract-signalResource, cloudformation.changeset-update-type-on-new-stack-fails, cloudformation.invalid-stack-name-rejected.
