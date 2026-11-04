Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 3157dd4d1552792a0982de1a1298281fbb744f2b

# CloudFormation adds none of its three aws:cloudformation:* system tags to a stack-created AWS::Lambda::Function, so lambda ListTags returns only the template's user tags

> _Note: this issue was identified against the code base at commit `3157dd4d1552792a0982de1a1298281fbb744f2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTags

## Description
Cause 1 of 1: resources provisioned by a CloudFormation stack do not carry CloudFormation's reserved system tags. Observed on AWS::Lambda::Function, read back through lambda ListTags.

AWS: CloudFormation stamps every taggable resource it creates with exactly three reserved tags — `aws:cloudformation:stack-name` (the name of the stack that owns the resource), `aws:cloudformation:stack-id` (that stack's full stack ARN/id) and `aws:cloudformation:logical-id` (the resource's logical id in the template) — in addition to any Tags the template declares. These are system tags: they are written by the provisioning service, not by the caller, and no caller can remove them (an UntagResource of a user tag leaves them in place). Smithy (lambda, com-amazonaws-lambda-awsgirapiservice): `ListTags` GET /2017-03-31/tags/{Resource} returns `Tags` (map<String,String>) with HTTP 200; the model does not distinguish system from user tags, so the three reserved keys must appear as ordinary entries of that map. The system-tag rule itself is CloudFormation-documented behaviour, not modelled — recorded behaviour confirms it: a stack with a `TaggedFunction` (template Tags `lambda-console:blueprint=true`) and an untagged `PlainFunction` yields, for each function, a `Tags` map whose `aws:cloudformation:`-prefixed subset is exactly those three keys, with `aws:cloudformation:stack-name` equal to the deployed stack's name and `aws:cloudformation:logical-id` equal to `TaggedFunction` / `PlainFunction` respectively.

Emulator: the stack reaches CREATE_COMPLETE and publishes both function ARNs, and the template's own user tag does surface (`lambda-console:blueprint=true` is present on the tagged function and absent on the plain one), but the `aws:cloudformation:`-prefixed subset of the returned `Tags` map is EMPTY for both functions. The check fails in its deploy phase with `AssertionError: [phase 1/4 deploy] CloudFormation must add exactly its three system tags to the tagged function; got []`. Nothing diverges in status or error code — ListTags answers 200 with a map that is simply missing the three reserved entries.

Why: a client that uses the CloudFormation system tags to attribute ownership gets nothing back. Stack-scoped inventory ("which functions belong to stack X"), drift/ownership tooling, cost-allocation and tag-based policies keyed on `aws:cloudformation:stack-name` or `aws:cloudformation:logical-id` all see untagged resources and conclude the stack owns nothing. The downstream claim in the same scenario — that the system tags survive a caller's UntagResource of the user tag, and that Resource Groups Tagging GetResources returns the system tags alongside the user tag in each ResourceTagMappingList entry — is unobservable for the same reason.

Fix: the CloudFormation provisioning path that materialises a template resource and writes its tag set — the step that merges the template's `Tags` property into the owning service's tag state for AWS::Lambda::Function. It must additionally inject the three reserved keys, with values taken from the stack being deployed (its name and its stack id) and from the template (the resource's logical id), for EVERY taggable resource it creates, including resources whose template declares no Tags at all (the plain sibling must carry the three system tags and nothing else). Lambda's own tag state and ListTags serializer need no change — they already round-trip the user tag. Whatever path CloudFormation uses to write tags must also mark these keys as non-removable, so a caller's lambda UntagResource of a user tag does not drop them.

Traces: lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle (deploy phase, 1 of 4).

Related but distinct open issues (none covers this cause): 20261024T113144Z-lambda-function-tags-are-absent-from-the-resource.md and 20261102T093929Z-lambda-resourcegroupstaggingapi-lambda-tag-plane-and-the-resource-groups-tagging.md concern lambda tags not reaching the tagging index (here the lambda-side read is reached and answers, just without the system tags); 20261103T171935Z-s3-getbuckettagging-omits-the-aws-cloudformation-tags-set.md, 20261104T173700Z-s3-getbuckettagging-returns-no-aws-cloudformation-reserved-tags.md, 20261103T171935Z-s3vectors-s3-vectors-listtagsforresource-omits-the-aws-cloudformation-tags.md and 20261103T171935Z-states-step-functions-state-machines-outlive-their-stack-and.md report the same missing system tags for other services' resources.

## Evidence
Check: lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle [mismatch]

Emulator (actual):
  AssertionError: [phase 1/4 deploy] CloudFormation must add exactly its three system tags to the tagged function; got []
  — i.e. lambda ListTags(Resource=<tagged function ARN>) -> 200 {"Tags": {"lambda-console:blueprint": "true"}}
    with no aws:cloudformation:* entries. The same emptiness holds for the untagged sibling.

AWS (expected), for each function the stack created:
  lambda ListTags(Resource=<function ARN>) -> 200 with a Tags map whose
  aws:cloudformation:-prefixed keys are exactly:
    aws:cloudformation:logical-id  = <the resource's logical id in the template>  (TaggedFunction / PlainFunction)
    aws:cloudformation:stack-name  = <the deployed stack's name>
    aws:cloudformation:stack-id    = <that stack's id>
  plus, for the tagged function only, the template's user tag lambda-console:blueprint=true
  (the plain sibling must carry the three system tags and no lambda-console:blueprint key).
  Values of stack-name/stack-id/logical-id are per-request/per-stack values, not fixed text; only the
  three key names and the rule that supplies their values are the requirement.

The assertion that passed immediately before proves the user tag path works: the template's Tags list
did surface on the tagged function and was absent from the plain one. Only the CloudFormation-injected
system tags are missing. Because the deploy phase fails, the later claims in the same scenario are
never reached: that the system tags survive lambda UntagResource of the user tag, and that
resourcegroupstaggingapi GetResources returns aws:cloudformation:logical-id=TaggedFunction alongside
the user tag in the matched ResourceTagMappingList entry.

## Steps to Reproduce
1. Create a CloudFormation stack from a template declaring an IAM exec role plus two AWS::Lambda::Function resources with logical ids TaggedFunction and PlainFunction (inline ZipFile code, python3.12, handler index.handler). Give TaggedFunction Properties.Tags = [{"Key": "lambda-console:blueprint", "Value": "true"}]; give PlainFunction no Tags. Output both function ARNs via Fn::GetAtt.
2. Wait for the stack to reach CREATE_COMPLETE and read TaggedFunctionArn / PlainFunctionArn from DescribeStacks Outputs.
3. Call lambda ListTags(Resource=<TaggedFunctionArn>) and lambda ListTags(Resource=<PlainFunctionArn>).
4. Filter each returned Tags map to keys starting with "aws:cloudformation:".
   Expected (AWS): exactly {aws:cloudformation:logical-id, aws:cloudformation:stack-id, aws:cloudformation:stack-name}, with logical-id = TaggedFunction / PlainFunction and stack-name = the stack's name.
   Actual (emulator): empty for both functions; only the template's user tag is present on the tagged one.
5. (Blocked by step 4) lambda UntagResource(Resource=<TaggedFunctionArn>, TagKeys=["lambda-console:blueprint"]) and re-read: the three aws:cloudformation:* keys must still be present. Then resourcegroupstaggingapi GetResources(TagFilters=[{Key:"lambda-console:blueprint"}], ResourceTypeFilters=["lambda:function"]) must return the system tags alongside the user tag in the matched entry.
