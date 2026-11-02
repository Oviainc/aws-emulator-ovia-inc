Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs__ssm.cdk-default-deploy-legacy-synthesizer-carries-cdk-metadata-without-bootstrap cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes cloudformation__sqs__ssm.change-set-disable-rollback-execute-leaves-create-failed-and-is-updatable
Issue-Key: cloudformation__sqs__ssm.cdk-default-deploy-legacy-synthesizer-carries-cdk-metadata-without-bootstrap|run#13|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-800:]!r}"
Issue-Key: cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes|run#19|AssertionError|assert (bad["ResourceStatus"], bad.get("ResourceStatusReason")) == (
Issue-Key: cloudformation__sqs__ssm.change-set-disable-rollback-execute-leaves-create-failed-and-is-updatable|run#21|AssertionError|assert (bad["ResourceStatus"], bad.get("ResourceStatusReason")) == (
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# cloudformation__sqs__ssm: 2 net-new causes — AWS::CDK::Metadata is refused for the legacy synthesizer too, and a failed resource's event carries the provider's wrapped message instead of CloudFormation's own reason

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Two net-new root causes.

Cause 1 of 2: AWS::CDK::Metadata is refused as an unrecognized resource type, so even a legacy-synthesizer CDK deploy (no bootstrap) cannot create its change set.
AWS: AWS::CDK::Metadata is a known, inert CloudFormation resource type; a legacy-synthesizer template carrying it plus a queue is accepted and deployed, and the metadata resource is listed on the stack. ValidationError 'Template format error: Unrecognized resource types: [...]' is reserved for types CloudFormation does not know. Smithy (cloudformation CreateChangeSet) models no error for a known type — recorded behavior only.
Emulator: '`cdk deploy` exited 1; output tail ... creating CloudFormation changeset...\nTemplate format error: Unrecognized resource types: [AWS::CDK::Metadata]'.
Why: no CDK deploy of any synthesizer flavour works against the emulator.
Fix: register AWS::CDK::Metadata as a no-op type in the CloudFormation resource-type registry / template validator.
Traces: cloudformation__sqs__ssm.cdk-default-deploy-legacy-synthesizer-carries-cdk-metadata-without-bootstrap.

Cause 2 of 2: the ResourceStatusReason of a resource that failed because the underlying object already exists is the resource provider's raw error wrapped in handler scaffolding, not CloudFormation's own reason.
AWS: when a stack resource collides with an object that already exists outside it, the CREATE_FAILED stack event's ResourceStatusReason is CloudFormation's own text '<physical name> already exists in stack <stack id>' — fixed text with two variable parts taken from the request and from state (the colliding physical name, the owning stack's id). Smithy (cloudformation DescribeStackEvents, member ResourceStatusReason) models the member but not its text, so the wording is recorded behavior only.
Emulator: the event instead carries the provider-style wrapper 'Resource handler returned message: "Parameter <name> already exists; set Overwrite to update it" (RequestToken: <uuid>, HandlerErrorCode: AlreadyExists)'. The status itself (CREATE_FAILED) is right; the reason is a different sentence, names no owning stack, and leaks a per-request token into a value clients compare on. Both the `cdk deploy --no-rollback` leg and the change-set disable-rollback leg show it with only the parameter name differing.
Why: a client that reads ResourceStatusReason to find which stack already owns a colliding resource gets a provider message that does not name it, and anything matching AWS's documented sentence fails.
Fix: the stack-event reason builder for already-exists resource collisions in CloudFormation's provisioner (the provider error must be mapped to CloudFormation's own reason, not re-emitted verbatim).
Traces: cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes, cloudformation__sqs__ssm.change-set-disable-rollback-execute-leaves-create-failed-and-is-updatable.

## Evidence
Cause 1 — Actual: '`cdk deploy` exited 1; output tail ... Template format error: Unrecognized resource types: [AWS::CDK::Metadata]'. Expected: the deploy succeeds without a bootstrap stack.
Cause 2 — Expected (ResourceStatus, ResourceStatusReason) == ('CREATE_FAILED', '<parameter name> already exists in stack arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>'). Actual: ('CREATE_FAILED', 'Resource handler returned message: "Parameter /sim2real-...-keep-... already exists; set Overwrite to update it" (RequestToken: ..., HandlerErrorCode: AlreadyExists)') on both legs.

## Steps to Reproduce
Cause 1: `cdk deploy` a legacy-synthesizer app (queue only, no bootstrap). Cause 2: create an object (e.g. an SSM parameter) outside the stack, then deploy a stack declaring the same physical name with --no-rollback (and separately via CreateChangeSet with DisableRollback + ExecuteChangeSet); read DescribeStacks (StackStatus/StackStatusReason/DisableRollback) and the failed resource's last DescribeStackEvents entry. Replay checks cloudformation__sqs__ssm.cdk-default-deploy-legacy-synthesizer-carries-cdk-metadata-without-bootstrap, cloudformation__sqs__ssm.cdk-deploy-no-rollback-keeps-create-failed-then-fixed-redeploy-completes, cloudformation__sqs__ssm.change-set-disable-rollback-execute-leaves-create-failed-and-is-updatable.
