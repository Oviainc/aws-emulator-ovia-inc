Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.continue-update-rollback-on-stable-stack-fails cloudformation.delete-stack-duplicate-token-rejected cloudformation.signal-resource-completes-wait-condition cloudformation.stack-continue-update-rollback-lifecycle cloudformation.stack-update-cancel-then-continue-rollback cloudformation.wait-condition-count-and-data-signal-accounting cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing cloudformation.validate-template-unsupported-format-version-rejected cloudformation.wait-condition-and-creation-policy-timeout-reasons cloudformation.cdk-rebootstrap-with-a-custom-template-updates-the-toolkit-stack-in-place
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.continue-update-rollback-on-stable-stack-fails|_routed_request#19|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.delete-stack-duplicate-token-rejected|_routed_request#23|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.signal-resource-completes-wait-condition|_routed_request#9|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.stack-continue-update-rollback-lifecycle|_routed_request#15|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.stack-update-cancel-then-continue-rollback|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.wait-condition-count-and-data-signal-accounting|_routed_request#14|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Key: cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing|_client_error#21|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudformation.validate-template-unsupported-format-version-rejected|_error#2|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudformation.wait-condition-and-creation-policy-timeout-reasons|run#15|AssertionError|assert policy_reasons == ["Failed to receive 1 resource signal(s) within the specified duration"], (
Issue-Key: cloudformation.cdk-rebootstrap-with-a-custom-template-updates-the-toolkit-stack-in-place|run#22|AssertionError|assert (code, message) == (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation: 5 net-new causes — custom-resource ResponseURL and WaitConditionHandle Ref are published on hosts the emulator does not serve, ValidateTemplate accepts templates AWS rejects, CreationPolicy timeout reason diverges, CreateChangeSet refuses a valid bootstrap template

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five distinct root causes in the CloudFormation control plane. Each is numbered with its own failing check ids; all five must be closed before this issue is done.

Cause 1 of 5: the custom-resource ResponseURL CloudFormation publishes names a host the emulator does not serve.
AWS: recorded behavior only — CloudFormation hands a custom resource a presigned ResponseURL that the caller can PUT to. In an emulator the only reachable address is the emulator's own host and port; a published URL must therefore be http(s)://<the emulator's own host>:<port>/<resource path>.
Emulator: the ResponseURL is published as `https://cloudformation-custom-resource-response.<region>.amazonaws.com/<url-encoded stack arn>|<logical id>|<token>?X-Amz-Algorithm=...&X-Amz-Signature=...`, a host that is not one of the emulator's own hosts, so the response PUT is refused before it leaves.
Why: no custom resource, wait-condition probe or continue-update-rollback flow can ever signal back, so those stacks hang or fail for a reason unrelated to the template.
Fix: the custom-resource ResponseURL minting in the CloudFormation provisioner — it must be built from the listener the request arrived on.
Traces: cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.delete-stack-duplicate-token-rejected, cloudformation.signal-resource-completes-wait-condition, cloudformation.stack-continue-update-rollback-lifecycle, cloudformation.stack-update-cancel-then-continue-rollback. (The open issue 20261109T185037Z-cloudformation-sns-sqs-6-net-new-causes-custom-resource-responseurl-points... reports the same shape under the cloudformation__sns__sqs heading; these five checks are not carried by it.)

Cause 2 of 5: Ref on AWS::CloudFormation::WaitConditionHandle is not a URL at all.
AWS: Ref on a WaitConditionHandle returns a presigned https URL that a signaller PUTs its status to.
Emulator: Ref returns the bare generated physical name `<stack-name>-<logical-id>-<suffix>`, which is then used as an endpoint and rejected because it has no scheme and no emulator host.
Why: a template that signals a wait condition by curling the handle Ref cannot work; the stack's signal accounting is unreachable.
Fix: the AWS::CloudFormation::WaitConditionHandle handler's Ref/physical-id resolution plus the signal listener it must point at.
Traces: cloudformation.wait-condition-count-and-data-signal-accounting. (Related open issue: 20261029T214321Z-cloudformation-aws-cloudformation-waitconditionhandle-ref-returns-the-logical.md, which carries different checks.)

Cause 3 of 5: ValidateTemplate accepts templates AWS rejects.
AWS: Smithy: ValidateTemplate takes TemplateBody/TemplateURL and declares no modelled errors, so refusals arrive as the service-common `ValidationError` at HTTP 400. AWS refuses a template whose Outputs reference an intrinsic that cannot be resolved, and refuses an AWSTemplateFormatVersion value other than a supported one.
Emulator: both calls return HTTP 200 with an empty `Parameters` list and no error.
Why: a client that uses ValidateTemplate as its pre-flight gate is told a broken template is fine and only discovers the problem at deploy time, or not at all.
Fix: ValidateTemplate's template parser/validator in the CloudFormation handler.
Traces: cloudformation.unresolvable-output-intrinsic-fails-the-stack-rather-than-vanishing, cloudformation.validate-template-unsupported-format-version-rejected.

Cause 4 of 5: a CreationPolicy timeout is reported with the WaitCondition wording.
AWS: a resource whose CreationPolicy ResourceSignal times out fails with the CreationPolicy's own reason naming the signal count it received and expected for that resource; the WaitCondition wording `WaitCondition timed out. Received <n> conditions when expecting <m>` belongs to AWS::CloudFormation::WaitCondition, not to a CreationPolicy resource.
Emulator: the CreationPolicy resource's failure reason is `WaitCondition timed out. Received 0 conditions when expecting 1`.
Why: a client cannot tell a CreationPolicy signal timeout from a WaitCondition timeout, and tooling that matches on the reason misclassifies the failure.
Fix: the CreationPolicy ResourceSignal timeout path in the CloudFormation provisioner's failure-reason generation.
Traces: cloudformation.wait-condition-and-creation-policy-timeout-reasons.

Cause 5 of 5: CreateChangeSet refuses a valid CDK bootstrap template as having unresolved Outputs dependencies.
AWS: `cdk bootstrap --template <custom template>` creates a change set over the existing toolkit stack and updates it in place; the Outputs block of that template resolves.
Emulator: CreateChangeSet answers HTTP 400 `ValidationError` with `Template format error: Unresolved resource dependencies [<logical id>] in the Outputs block of the template`, so the re-bootstrap never runs. The bracketed logical id comes from the template; the rest is fixed text.
Why: a CDK user cannot re-bootstrap with a custom toolkit template, which blocks every deployment flow that depends on a non-default bootstrap.
Fix: the template Outputs dependency resolution used by CreateChangeSet.
Traces: cloudformation.cdk-rebootstrap-with-a-custom-template-updates-the-toolkit-stack-in-place.

## Evidence
Cause 1: simparity.published.PublishedUrlRejected: endpoint_url was given https://cloudformation-custom-resource-response.us-west-2.amazonaws.com/arn%3Aaws%3Acloudformation%3A...%7CProbe%7C...?X-Amz-Algorithm=AWS4-HMAC-SHA256&... — 'its host is not one of the emulator's own hosts'.
Cause 2: simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-wc-count2-...-Handle-... — not an emulator host (and not a URL path at all).
Cause 3: 'real AWS rejects this call, but it SUCCEEDED and returned {'Parameters': [], ... 'HTTPStatusCode': 200 ...}' for both ValidateTemplate calls.
Cause 4: "the CreationPolicy resource's failure reason differs: ['WaitCondition timed out. Received 0 conditions when expecting 1']".
Cause 5: "CreateChangeSet with the broken template answered ('ValidationError', 'Template format error: Unresolved resource dependencies [DoesNotExistParamOrResource] in the Outputs block of the template')".

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: deploy a stack with an AWS::CloudFormation::CustomResource (or a ResourceSignal probe) and inspect the ResponseURL handed to it.
3. Cause 2: deploy AWS::CloudFormation::WaitConditionHandle and read its Ref.
4. Cause 3: call ValidateTemplate with a template whose Outputs use an unresolvable intrinsic, and with AWSTemplateFormatVersion set to an unsupported value.
5. Cause 4: deploy a resource with a CreationPolicy ResourceSignal that never arrives and read the resource's CREATE_FAILED reason.
6. Cause 5: run `cdk bootstrap --template <custom template>` over an existing toolkit stack.
