Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.stack-instance-and-operation-not-found-errors cloudformation.type-registry-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.stack-instance-and-operation-not-found-errors|_expect_error#25|AssertionError|assert got_msg == message, f"expected message {message!r}, got {got_msg!r}"
Issue-Key: cloudformation|PublishType|CFNRegistryException
Issue-Commit: 9a43c5c780b0b7aa4996b192046c21b48dc59a9c

# CloudFormation: 2 net-new root causes — StackSet OperationNotFoundException message text, and PublishType checks ARN version shape before type existence

> _Note: this issue was identified against the code base at commit `9a43c5c780b0b7aa4996b192046c21b48dc59a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackSetOperation

## Description
Two net-new behavioral root causes in the CloudFormation emulator. Both are refusal-fidelity defects: the HTTP status and the error code are right, but the Error.Message a client reads is not the message AWS produces for that error class. Each cause is independent and must be closed separately.

Neither cause is covered by an existing open issue: the replay report marks both checks as neither ALREADY FILED nor OWNED, and `list_existing_issues` has no issue naming the StackSet operation-not-found message or PublishType's refusal ordering. The two other type-registry gaps in this suite (`SetTypeDefaultVersion`, `DescribeTypeRegistration`, `RegisterType`) are separate scaffold-stub issues already filed for us and are NOT what this reports.

=== Cause 1 of 2: operation-scoped StackSet APIs return a non-AWS OperationNotFoundException message ===

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models `OperationNotFoundException` (errorKind client, httpError 404, awsQueryError code OperationNotFoundException / 404) on DescribeStackSetOperation, ListStackSetOperationResults and StopStackSetOperation, each taking `OperationId` (target ClientRequestToken, @length 1..128, pattern ^[a-zA-Z0-9][-a-zA-Z0-9]*$). The model does not carry message text, so the message is recorded behavior only: for an OperationId that names no operation of an existing StackSet, AWS answers HTTP 404, Error.Code `OperationNotFoundException`, Error.Message exactly `Operation not found`. That message is ENTIRELY fixed text — it interpolates nothing from the request, neither the OperationId nor the StackSet name.

Emulator: same status and code, but the message is built by interpolating the request's OperationId into a different template, e.g. `StackSet operation fddff6b8-1e14-4b61-9df1-35c1ef54ee40 not found` (the uuid is the per-run OperationId the check generated, not part of the requirement; the diverging part is the template `StackSet operation <OperationId> not found` in place of the fixed `Operation not found`).

Why: a client that classifies or asserts on Error.Message — error-taxonomy tests, log scrapers, retry/branch logic keyed to the documented refusal string, and CLI output compared against AWS — sees a message AWS never emits. Because the emulator's variant embeds the request's OperationId, it is also not stable across runs, so any equality comparison against it is unreproducible as well as wrong.

Fix: the shared not-found refusal path for operation-scoped StackSet operations — the error construction used by the DescribeStackSetOperation, ListStackSetOperationResults and StopStackSetOperation handlers in the cloudformation service's stack-set ops/validation layer. It must emit the fixed string `Operation not found` with no interpolation, while leaving the StackSet-level refusal (`StackSet <name> not found`, which the same check asserts and which already passes) untouched.

Traces: cloudformation.stack-instance-and-operation-not-found-errors — the check's StackSet-level StackSetNotFoundException assertions, the SELF_MANAGED ListStackSetAutoDeploymentTargets ValidationError, the StackInstanceNotFoundException assertion and the positive-control UpdateStackSet/DescribeStackSetOperation SUCCEEDED poll all pass; it fails at the first DescribeStackSetOperation call with an unknown OperationId.

=== Cause 2 of 2: PublishType validates the ARN's version segment before checking the type exists in the account ===

AWS: Smithy models PublishType with `Arn` (target PrivateTypeArn, @length 0..1024, pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$) and errors `CFNRegistryException` (client, httpError 400) and `TypeNotFoundException` (client, httpError 404). The pattern admits any `type/...` suffix, so a private type ARN carrying a trailing version segment is a well-formed input and gets past input validation. Message text is recorded behavior only: for a private type ARN in an account/region where no such extension type is registered, AWS answers Error.Code `CFNRegistryException` with Error.Message exactly `Type not found in your account.` — fixed text, no interpolation of the ARN, type name or version. Existence/ownership is therefore resolved BEFORE any rule about which version may be published.

Emulator: for the same request it answers Error.Code `CFNRegistryException` with Error.Message `Only default version can be published. Please remove versionId from Arn` — also entirely fixed text, but the message of a different refusal class (the version-shape rule). The ordering is inverted: the emulator applies the versionId rule to the ARN string first and never reaches the account-scoped existence lookup, so it reports a malformed-request reason for a type that simply is not registered.

Why: a client cannot tell "this type is not in my account" from "this ARN names a non-default version". Registry tooling that branches on the refusal reason will strip the version segment and retry — and get the same class of failure for an unregistered type, since the real problem was never the ARN. Error-taxonomy assertions and operator diagnostics both read the wrong cause.

Fix: the PublishType handler's validation ordering in the cloudformation service's type-registry ops — resolve the private type ARN against the account's registered types first and refuse with CFNRegistryException / `Type not found in your account.` when nothing is registered; apply the default-version rule only to an ARN that does resolve to a registered type.

Traces: cloudformation.type-registry-not-found-errors — the DescribeType positive control on AWS::S3::Bucket, the TypeNotFoundException assertions for DescribeType/DeactivateType/DeregisterType and the ActivateType `TypeName is invalid` CFNRegistryException all pass; it fails at the PublishType call with a private type ARN for an unregistered type.

## Evidence
Cause 1 — check cloudformation.stack-instance-and-operation-not-found-errors:
  expected: Error.Code 'OperationNotFoundException', Error.Message 'Operation not found' (HTTP 404)
  actual:   Error.Code 'OperationNotFoundException', Error.Message 'StackSet operation fddff6b8-1e14-4b61-9df1-35c1ef54ee40 not found'
  assertion: AssertionError: expected message 'Operation not found', got 'StackSet operation fddff6b8-1e14-4b61-9df1-35c1ef54ee40 not found'
  (the uuid is the per-run generated OperationId; the requirement is the fixed string 'Operation not found' with NO interpolated value. Same refusal is asserted for ListStackSetOperationResults and StopStackSetOperation with the same unknown OperationId.)
  Smithy: DescribeStackSetOperation / ListStackSetOperationResults / StopStackSetOperation -> OperationNotFoundException, errorKind client, httpError 404, awsQueryError {code: OperationNotFoundException, httpResponseCode: 404}.

Cause 2 — check cloudformation.type-registry-not-found-errors:
  expected: ('CFNRegistryException', 'Type not found in your account.')
  actual:   ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn')
  assertion: AssertionError: expected ('CFNRegistryException', 'Type not found in your account.'), got ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn')
  request: PublishType(Arn='arn:aws:cloudformation:<region>:<unregistered-account>:type/resource/Sim2realBehav-Probe-<token>/00000001') — a well-formed PrivateTypeArn per the modeled pattern, naming a type registered in no account.
  Smithy: PublishType errors = CFNRegistryException (client, httpError 400), TypeNotFoundException (client, httpError 404); Arn pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$ (a trailing version segment is admitted).
  Both messages are fixed text; the divergence is which refusal class is reached, i.e. validation ordering.

## Steps to Reproduce
Cause 1:
1. cloudformation.CreateStackSet(StackSetName=<fresh name>, TemplateBody=<single AWS::SNS::Topic>) and let it settle (zero stack instances).
2. Call DescribeStackSetOperation(StackSetName=<that name>, OperationId=<a fresh uuid that is not an operation of the stack set>).
3. Observe HTTP 404 / Error.Code OperationNotFoundException with Error.Message 'StackSet operation <uuid> not found'. AWS returns the fixed message 'Operation not found'.
4. Repeat with ListStackSetOperationResults and StopStackSetOperation using the same unknown OperationId — same divergence.

Cause 2:
1. Build a private type ARN for a type nobody registered: arn:aws:cloudformation:<region>:<12-digit account that owns no such type>:type/resource/Sim2realBehav-Probe-<token>/00000001
2. Call cloudformation.PublishType(Arn=<that ARN>).
3. Observe Error.Code CFNRegistryException with Error.Message 'Only default version can be published. Please remove versionId from Arn'. AWS answers CFNRegistryException with 'Type not found in your account.' because it resolves existence/ownership before applying the default-version rule.
4. Re-run the full checks to confirm: cloudformation.stack-instance-and-operation-not-found-errors and cloudformation.type-registry-not-found-errors.
