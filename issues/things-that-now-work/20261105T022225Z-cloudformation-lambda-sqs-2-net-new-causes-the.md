Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals|run#14|AssertionError|assert physical_id.startswith("sim2real-xsvc-cr-"), (
Issue-Key: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# cloudformation__lambda__sqs: 2 net-new causes — the custom-resource response body's PhysicalResourceId is not adopted, and a FAILED custom-resource response does not roll the stack back

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes in the CloudFormation custom-resource protocol. Neither failing check is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: CloudFormation ignores the PhysicalResourceId the custom-resource responder returns.
AWS: recorded behavior only (CloudFormation custom-resource response protocol). The provider PUTs a response document containing PhysicalResourceId; CloudFormation adopts that exact string as the resource's physical id, so `{"Ref": "<CustomResourceLogicalId>"}` — and Outputs built from it — return the responder-minted id, and the DELETE request CloudFormation later sends echoes the same id.
Emulator: Ref of the custom resource returns an emulator-generated name of the form `<stack-name>-<logical-id>-<suffix>`, i.e. the responder's PhysicalResourceId is discarded.
Why: a client whose provider mints a meaningful id (an external record key, a tenant id, a resource handle) cannot read it back from the stack, and cannot correlate the DELETE request with what it created — the create/delete lifecycle signalling that custom resources exist for is unobservable.
Fix: the CloudFormation custom-resource response handling — the stored PhysicalResourceId for a Custom::* resource must come from the response body, and the Ref/Outputs resolution must return it.
Traces: cloudformation__lambda__sqs.cfn-custom-resource-create-delete-lifecycle-signals.

Cause 2 of 2: a custom resource that answers Status FAILED is treated as a success.
AWS: recorded behavior only. When the provider PUTs a response whose Status is "FAILED" (with a Reason), CloudFormation marks that resource CREATE_FAILED, records the Reason as the resource's ResourceStatusReason, and drives the stack through ROLLBACK_IN_PROGRESS to ROLLBACK_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE — the response Status is ignored entirely.
Why: a provider's own failure signal has no effect. A client that validates inputs or external preconditions inside a custom resource and reports FAILED gets a green stack over a half-provisioned estate, and no rollback is ever observed.
Fix: the CloudFormation custom-resource response handling (Status/Reason -> resource CREATE_FAILED) and the stack state machine's rollback transition.
Traces: cloudformation__lambda__sqs.cfn-custom-resource-failed-response-rollback-negative.

## Evidence
Cause 1 — expected: Ref of the Custom:: resource is the responder-minted physical id (the check asserts it starts with the responder's own prefix). Actual: `AssertionError: Ref of the custom resource returned '<stack>-Recorded-<suffix>', expected the responder-minted physical id (CFN must adopt the id from the response body)`.

Cause 2 — expected: stack status ROLLBACK_COMPLETE after the provider answers Status FAILED. Actual: `AssertionError: a custom resource that answered FAILED left the stack in CREATE_COMPLETE, expected ROLLBACK_COMPLETE (CREATE_COMPLETE here means the response Status was ignored)`.

(Stack/resource suffixes in both messages are per-run values; the adopted-id rule and the status sequence are the requirements.)

## Steps to Reproduce
1. CreateStack with a template declaring a provider Lambda, an AWS::Lambda::Permission for cloudformation.amazonaws.com, and a Custom::Recorded resource whose ServiceToken is that function; the handler PUTs a response document with Status SUCCESS and a PhysicalResourceId of its own choosing.
2. Wait for CREATE_COMPLETE and resolve `{"Ref": "Recorded"}` (via a stack Output or DescribeStackResources) — the value is `<stack>-Recorded-<suffix>`, not the id the handler returned (cause 1).
3. Repeat with a handler that PUTs Status "FAILED" and poll DescribeStacks for the terminal status — it is CREATE_COMPLETE, not ROLLBACK_COMPLETE (cause 2).
