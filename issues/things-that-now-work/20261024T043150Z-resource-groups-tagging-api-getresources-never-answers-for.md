Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resourcegroupstaggingapi__stepfunctions.state-machine-tags-visible-through-get-resources
Issue-Key: resourcegroupstaggingapi__stepfunctions.state-machine-tags-visible-through-get-resources|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# resource-groups-tagging-api GetResources never answers for a tagged Step Functions state machine

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources

## Description
Cause 1 of 1: GetResources does not return the tagged state machine (the call never produces a usable response).
AWS: a state machine created with `tags` (or tagged via sfn:TagResource) is discoverable through the Resource Groups Tagging API: `GetResources` with `ResourceTypeFilters=['states']` or a `TagFilters` entry for the tag key returns a `ResourceTagMappingList` entry whose `ResourceARN` is the state machine ARN and whose `Tags` carry the tag set.
Emulator: the check polling GetResources for that mapping never sees it and is killed by the harness with `Check timed out after 60.0s`.
Why: tag-driven inventory, cost-allocation and cleanup tooling finds nothing for Step Functions resources, and a client polling for a tag to appear blocks indefinitely rather than getting an answer it can act on.
Fix: the GetResources handler in the resource-groups-tagging-api crate and the cross-service tag index it reads (the sfn tag state must be enumerated under the `states` resource type).
Traces: resource-groups-tagging-api__sfn.state-machine-tags-visible-through-get-resources.

## Evidence
Expected: a ResourceTagMappingList entry for the state machine ARN carrying the tags it was created with. Actual: no such entry within the harness cap — 'Check timed out after 60.0s'.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. sfn:CreateStateMachine with tags=[{'key':'k','value':'v'}].
3. resourcegroupstaggingapi:GetResources with ResourceTypeFilters=['states'] (and with TagFilters=[{'Key':'k'}]) and look for the state machine's ARN.
