Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.e2e-maintenance-window-execution-history-flow ssm.maintenance-window-cutoff-blocks-task-execution-near-end ssm.maintenance-window-execution-task-invocation-describe-chain ssm.maintenance-window-execution-task-invocation-times-out ssm.list-tags-resolves-parameter-id-forms-and-version-selector-rejected ssm.put-parameter-create-tags-survive-overwrites ssm.put-parameter-overwrite-with-tags-rejected ssm.ssm-automation-branch-takes-one-path-and-the-other-branchs-effect-is-absent
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.e2e-maintenance-window-execution-history-flow|_poll_until#2|AssertionError|raise AssertionError(f"condition never satisfied, last value={value!r}")
Issue-Key: ssm.maintenance-window-cutoff-blocks-task-execution-near-end|_wait_for_agent_online#2|AssertionError|raise AssertionError(
Issue-Key: ssm.maintenance-window-execution-task-invocation-describe-chain|_launch_managed_instance#2|AssertionError|raise AssertionError(
Issue-Key: ssm.maintenance-window-execution-task-invocation-times-out|_poll_until#2|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last={last!r})")
Issue-Key: ssm|ListTagsForResource|InvalidResourceId
Issue-Key: ssm.put-parameter-create-tags-survive-overwrites|run#8|AssertionError|assert _tags(ssm, name) == created, (
Issue-Key: ssm.put-parameter-overwrite-with-tags-rejected|run#10|AssertionError|assert got == refusal, f"Overwrite+Tags on an existing name gave {got!r}"
Issue-Key: ssm.ssm-automation-branch-takes-one-path-and-the-other-branchs-effect-is-absent|run#15|AssertionError|assert left_run["AutomationExecutionStatus"] == "Success", (
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# ssm: 5 net-new causes — managed nodes never register (4 maintenance-window checks blocked), ListTagsForResource refuses valid parameter id forms, Overwrite drops a parameter's tags, the Overwrite+Tags refusal is emulator-authored, and aws:executeAwsApi branch steps never run

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Five net-new root causes, each grounded in its own failing checks under this heading.

Cause 1 of 5: an EC2 instance launched with an AmazonSSMManagedInstanceCore instance profile never becomes an Online SSM managed node, so every maintenance-window execution scenario stops at its setup gate.
AWS: such an instance registers with Systems Manager shortly after boot, and DescribeInstanceInformation returns an InstanceInformation row for it with PingStatus `Online`. Smithy (ssm): DescribeInstanceInformation output InstanceInformationList -> InstanceInformation{InstanceId, PingStatus: enum Online|ConnectionLost|Inactive, ...}; an unregistered instance is simply absent from the list — there is no modeled error, which is why this fails as a timeout rather than a refusal.
Emulator: DescribeInstanceInformation never returns a row for the instance (`last=None` / `last value=None`) for 420–600 s, so PingStatus never becomes Online. The four maintenance-window checks (execution history, cutoff near window end, the execution/task/invocation describe chain, and the invocation timeout) therefore never reach the behaviour they test.
Why: a client following AWS's documented path to a managed node never gets one, so Run Command, Session Manager, patching, inventory and every maintenance-window task that targets instances are unreachable. The failure is silent — an empty list, not an error.
Fix: the SSM managed-node registration path (registering an instance whose profile grants the SSM core policy and maintaining its InstanceInformation/PingStatus).
Traces: e2e-maintenance-window-execution-history-flow, maintenance-window-cutoff-blocks-task-execution-near-end, maintenance-window-execution-task-invocation-describe-chain, maintenance-window-execution-task-invocation-times-out.

Cause 2 of 5: ListTagsForResource refuses a valid parameter resource-id form with InvalidResourceId.
AWS: for ResourceType `Parameter`, ListTagsForResource accepts the parameter name with its leading slash, the same name without the leading slash, and the parameter ARN (`arn:aws:ssm:<region>:<account>:parameter/<name>`) — all three return the same TagList. InvalidResourceId is reserved for an id that is not a usable parameter id (e.g. a name carrying a version selector `<name>:1`, or the Document resource type for a parameter name), and AWS returns it with an empty message. Smithy (ssm): ListTagsForResource input {ResourceType: ResourceTypeForTagging (required), ResourceId: ResourceId (required)}; errors InvalidResourceId (400), InvalidResourceType (400).
Emulator: one of the three valid forms raises `InvalidResourceId` (400) with an empty message, aborting the check before the negative cases.
Why: a client that holds a parameter's ARN (the usual thing to hold — it is what GetParameter returns, and what IAM policies and CloudFormation outputs carry) cannot read that parameter's tags at all; tagging tools that resolve resources by ARN break on SSM parameters.
Fix: the ListTagsForResource (and AddTagsToResource/RemoveTagsFromResource) resource-id resolver for ResourceType=Parameter — it must normalize all three accepted id forms to the parameter name before looking up tags, and reject only version-selected ids and type mismatches.
Traces: list-tags-resolves-parameter-id-forms-and-version-selector-rejected.

Cause 3 of 5: PutParameter with Overwrite discards the parameter's tags.
AWS: tags supplied at create time belong to the parameter, not to a version. An overwrite that carries no Tags (or an empty Tags list) updates value and version and leaves the tag set untouched; ListTagsForResource still reports the create-time tags. Smithy (ssm): PutParameter input Tags -> TagList (optional, length 0..1000); nothing in the model ties the tag set to a version, and no error is modeled for an overwrite that omits tags.
Emulator: after an overwrite (including one with `Tags: []`) ListTagsForResource reports `[]` — the create-time tags are gone.
Why: a client loses cost-allocation, ownership and policy tags on every value rotation, silently. Tag-condition IAM policies that gate access to the parameter stop matching, so an update turns into an access outage; and `Tags: []` — explicitly documented as "no tags supplied" — behaves as "remove all tags".
Fix: the PutParameter overwrite path in the SSM parameter store — the parameter's tag set must be preserved across an overwrite that does not supply tags (and Overwrite with Tags must be refused, see cause 4).
Traces: put-parameter-create-tags-survive-overwrites.

Cause 4 of 5: the PutParameter Overwrite+Tags refusal message is emulator-authored.
AWS: PutParameter with both Overwrite and Tags is refused 400 `ValidationException` with the message `Invalid request: tags and overwrite can't be used together. To create a parameter with tags, please remove overwrite flag. To update tags for an existing parameter, please use AddTagsToResource or RemoveTagsFromResource.` — entirely fixed text with no request-derived parts — for an existing name and for a new one alike, leaving value, version and tags unchanged and creating nothing. Smithy (ssm): PutParameter's modeled errors contain no tags/overwrite error, so this is the service-common ValidationException; the wording is recorded behavior.
Emulator: the code is right (`ValidationException`) but the message is `Tags and Overwrite can't be used together; use AddTagsToResource to tag an existing parameter` — different fixed text, and it omits both the `Invalid request: ` prefix and the RemoveTagsFromResource remedy.
Why: a client that surfaces or matches the service's message shows wording AWS never emits, and remediation tooling keyed on the documented sentence does not fire.
Fix: the PutParameter validation in the SSM service — emit AWS's exact ValidationException text for the tags+overwrite conflict.
Traces: put-parameter-overwrite-with-tags-rejected.

Cause 5 of 5: an automation document's aws:executeAwsApi step is never run, so a branch that should succeed ends Failed with emulator prose.
AWS: `aws:executeAwsApi` calls the named AWS API from within the automation; a document whose aws:choice branch selects a step that reads/writes through aws:executeAwsApi reaches `AutomationExecutionStatus` = `Success`, the chosen branch's effect is present and the other branch's effect is absent. Smithy (ssm): GetAutomationExecution output AutomationExecution.AutomationExecutionStatus -> AutomationExecutionStatus enum (Pending|InProgress|Success|Failed|...); FailureMessage carries the step's failure.
Emulator: the execution ends `Failed` with FailureMessage `The aws:executeAwsApi action of step LEFT runs where this emulator does not reach, so nothing ran for this step` — emulator-authored prose for a step AWS executes; the step's whole fixed text has no AWS counterpart. The branch's effect is therefore absent and the branch semantics cannot be observed at all.
Why: a client cannot use Automation for anything that calls an AWS API — the single most common runbook primitive. Branching runbooks fail wholesale, and the failure message names the emulator rather than any condition a client can act on.
Fix: the SSM Automation step executor — aws:executeAwsApi must dispatch the named API call against the emulator's own services and feed its outputs into the step's outputs.
Traces: ssm-automation-branch-takes-one-path-and-the-other-branchs-effect-is-absent. Related but not covering: open issue 20261105T123903Z-ssm-automation-aws-executeawsapi-and-aws-executescript-steps.md reports the same executor gap for other checks; this failing check is not carried by it.

## Evidence
Cause 1: `AssertionError: instance i-<id> never reached PingStatus=Online within 600s (SSM Agent never registered)`; `AssertionError: EC2 instance i-<id> did not register as an SSM managed node within 8 minutes`; `AssertionError: condition never satisfied, last value=None`; `AssertionError: condition not met within 420.0s (last=None)`. Expected: DescribeInstanceInformation lists the instance with PingStatus='Online'.

Cause 2: `botocore.errorfactory.InvalidResourceId: An error occurred (InvalidResourceId) when calling the ListTagsForResource operation:` (empty message) for one of ResourceId = `/<name>`, `<name>` (no leading slash), or the parameter ARN. Expected: 200 with the parameter's TagList for all three forms; InvalidResourceId only for `<name>:1` and for ResourceType='Document'.

Cause 3: `AssertionError: after overwrite to 'v4' with ['Tags', 'Type'] tags list as []`. Expected: the create-time TagList (e.g. owner/tier) after every overwrite, including one with Tags=[].

Cause 4: expected ('ValidationException', 'Invalid request: tags and overwrite can\'t be used together. To create a parameter with tags, please remove overwrite flag. To update tags for an existing parameter, please use AddTagsToResource or RemoveTagsFromResource.'); actual ('ValidationException', "Tags and Overwrite can't be used together; use AddTagsToResource to tag an existing parameter").

Cause 5: `AssertionError: the Which=left execution must reach Success; got 'Failed' / 'The aws:executeAwsApi action of step LEFT runs where this emulator does not reach, so nothing ran for this step'`.

## Steps to Reproduce
Cause 1: launch a t3.micro with an instance profile whose role has AmazonSSMManagedInstanceCore; poll ssm DescribeInstanceInformation(Filters=[InstanceIds=<id>]) — the list stays empty and PingStatus is never Online.

Cause 2: ssm PutParameter(Name='/p/svc/key', Value='v1', Type='String', Tags=[...]); then ListTagsForResource(ResourceType='Parameter', ResourceId=...) for each of '/p/svc/key', 'p/svc/key' and the ARN from GetParameter — one of them raises InvalidResourceId with an empty message.

Cause 3: ssm PutParameter(Name=<n>, Value='v1', Type='String', Tags=[{owner,alpha},{tier,gold}]); then PutParameter(Name=<n>, Value='v2', Overwrite=True, Type='String'), …(Value='v3', Overwrite=True), …(Value='v4', Overwrite=True, Type='String', Tags=[]); ListTagsForResource after each — tags come back [].

Cause 4: ssm PutParameter(Name=<existing>, Value='v2', Type='String', Overwrite=True, Tags=[{owner,beta}]) — compare the ValidationException message with AWS's wording.

Cause 5: create an automation document with an aws:choice selecting between two steps, each an aws:executeAwsApi with an observable effect; StartAutomationExecution with Which=left; GetAutomationExecution — status is Failed with the emulator's step prose instead of Success.
