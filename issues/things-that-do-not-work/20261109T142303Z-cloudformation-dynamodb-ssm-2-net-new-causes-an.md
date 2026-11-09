Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__ssm.ssm-parameter-map-tags-reserved-prefix-rejected-rolls-back-stack cloudformation__dynamodb__ssm.cdk-bootstrap-version-gate-blocks-deploy-until-rebootstrapped
Issue-Key: cloudformation__dynamodb__ssm.ssm-parameter-map-tags-reserved-prefix-rejected-rolls-back-stack|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__dynamodb__ssm.cdk-bootstrap-version-gate-blocks-deploy-until-rebootstrapped|_cdk#43|Timeout|return result.returncode, output
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# cloudformation+dynamodb+ssm: 2 net-new causes — an aws:-prefixed key in an AWS::SSM::Parameter's Map-shaped Tags is accepted instead of failing the stack, and the CDK bootstrap-version gate scenario never settles

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes, both grounded in failing checks under this heading.

Cause 1 of 2: reserved `aws:` tag keys are not rejected when AWS::SSM::Parameter Tags are given in their Map (JSON-object) form, so a hostile key is silently accepted and the stack reaches CREATE_COMPLETE.
AWS: `aws:` is a reserved tag-key prefix — no customer call may create a tag whose key begins with `aws:`. A CloudFormation resource whose Tags contain such a key fails the resource create; the stack rolls back and ends ROLLBACK_COMPLETE, with the resource's CREATE_FAILED reason carrying the service's reserved-prefix refusal. AWS::SSM::Parameter accepts Tags in Map form (`{"key": "value"}`), and the reserved-prefix rule applies identically to the Map form and the list form. Smithy (ssm): PutParameter input Tags -> TagList; the reserved-prefix rule is a service rule applied to the key, not a modeled pattern, so this is recorded behavior for the refusal text and a documented AWS rule for the rejection itself.
Emulator: the stack reaches `CREATE_COMPLETE`; the parameter is created with the `aws:`-prefixed key and nothing rolls back.
Why: a client relying on the reserved prefix to be unforgeable can have account-level `aws:`-namespaced tags written by a template, which corrupts cost allocation and any policy that conditions on an `aws:` tag; and a template that should have been rejected deploys, so the mistake is only discovered from the tag data.
Fix: tag-key validation in the AWS::SSM::Parameter resource handler (and the shared tag normalizer that flattens the Map form into a TagList) — the reserved-prefix check must run on both input shapes, before the parameter is written.
Traces: cloudformation__dynamodb__ssm.ssm-parameter-map-tags-reserved-prefix-rejected-rolls-back-stack.

Cause 2 of 2: the CDK bootstrap-version gate scenario never reaches a verdict — the check is cut off at its 60 s harness budget with no observation, although its own bootstrap/deploy budgets are 900 s/600 s.
AWS: with a toolkit (CDKToolkit) stack whose `/cdk-bootstrap/<qualifier>/version` SSM parameter is below the app's `requiresBootstrapStackVersion`, `cdk deploy` refuses before creating any stack, naming both the found and the required version; after `cdk bootstrap` restores the version, the same deploy succeeds and `cdk destroy` removes the stack. The gate is read through ssm GetParameter on that bootstrap-version parameter. Recorded behavior only — the version-gate flow is a toolkit behaviour, not a modeled API error.
Emulator: `Check timed out after 60.0s` — nothing in the flow settles inside the harness budget, so neither the refusal nor the post-rebootstrap success is observable. Hypothesis: one of the toolkit-stack reads or the bootstrap-version parameter write in this flow does not complete, which is what stalls the whole check; the recording does not isolate which call, only that no step produced a response.
Why: a client cannot use the bootstrap-version gate at all — a downgraded or missing toolkit version neither refuses nor deploys, it hangs, which in CI looks like an infrastructure outage rather than a version mismatch.
Fix: the SSM parameter / CloudFormation toolkit-stack read path this flow drives (the `/cdk-bootstrap/<qualifier>/version` parameter and the CDKToolkit stack describe), which must answer promptly so the gate can be evaluated.
Traces: cloudformation__dynamodb__ssm.cdk-bootstrap-version-gate-blocks-deploy-until-rebootstrapped.

## Evidence
Cause 1 — expected: a template whose AWS::SSM::Parameter Properties.Tags is the Map `{"aws:hostile": "y"}` fails that resource and the stack ends `ROLLBACK_COMPLETE`.
Actual: `AssertionError: an aws:-prefixed key in the parameter's Map-shaped Tags should fail that resource and roll the stack back; got 'CREATE_COMPLETE'`. Positive control (Tags `{"param-resource-tag": "pv"}`) is expected to succeed and does.

Cause 2 — expected: `cdk deploy` refuses naming both versions with no stack created, then succeeds after `cdk bootstrap`, then `cdk destroy` removes the stack — all inside the check's 900 s/600 s budgets.
Actual: `Check timed out after 60.0s`; no step of the flow produced an observation.

## Steps to Reproduce
Cause 1:
1. CreateStack with resources {Table: AWS::DynamoDB::Table, Param: {Type: AWS::SSM::Parameter, Properties: {Name, Type: String, Value, Tags: {"aws:hostile": "y"}}}}.
2. Poll DescribeStacks — the stack reaches CREATE_COMPLETE; expected ROLLBACK_COMPLETE with the parameter resource CREATE_FAILED on the reserved tag key.
3. ssm ListTagsForResource(ResourceType='Parameter', ResourceId=<name>) shows the aws:-prefixed tag was written.

Cause 2:
1. `cdk bootstrap` a qualifier, then lower the toolkit's `/cdk-bootstrap/<qualifier>/version` SSM parameter below the app's requiresBootstrapStackVersion.
2. Run `cdk deploy` for an app that requires the higher version — the flow never settles (no refusal, no deploy) and the check is cut off at its harness budget.
