Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__resourcegroupstaggingapi.blueprint-tag-applied-after-create-surfaces-in-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-untag-removes-the-function-from-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-applied-after-create-surfaces-in-the-tag-query|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-untag-removes-the-function-from-the-tag-query|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resourcegroupstaggingapi|ListTags|ResourceNotFoundException
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Lambda function tags are absent from the Resource Groups Tagging index, and a CloudFormation-deployed AWS::Lambda::Function does not exist afterwards (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two causes, both in the Lambda surface.

Cause 1 of 2: lambda:function tags never enter the Resource Groups Tagging index, in either direction.
AWS: tags supplied to CreateFunction's Tags and tags added later with Lambda TagResource (Smithy: lambda#TagResource, POST /2017-03-31/tags/{Resource}, 204, input Resource (function ARN, @pattern arn:...:function:...) + Tags map; read back by lambda#ListTags, GET /2017-03-31/tags/{Resource}) land in the same account tag store that ResourceGroupsTaggingAPI GetResources reads (Smithy: resourcegroupstaggingapi#GetResources, TagFilters + ResourceTypeFilters, output ResourceTagMappingList of ResourceTagMapping{ResourceARN,Tags}). So a key-only TagFilters query narrowed to ResourceTypeFilters=["lambda:function"] selects exactly the functions carrying that key — including one tagged after creation — and Lambda UntagResource removes a function from that query while leaving its other keys indexed. Recorded behavior shows the index trailing a write by about 3 s.
Emulator: both checks poll that query to a bounded deadline and it never converges; each is killed at the 60 s harness budget. The create-time control function tagged lambda-console:blueprint=true is never selected either, so the query is not just missing the tag-after-create path — GetResources returns no lambda:function mappings at all. Lambda's own tag store and the tagging index are not the same store.
Why: the normal way to answer "which functions carry tag X" (the Tagging API, cross-region/cross-service) returns nothing, so tag-driven discovery, cost allocation and cleanup silently skip every Lambda function; and an untag can never be observed to retract an index entry, so a client that does get an answer cannot trust it to be current.
Fix: the Lambda CreateFunction/TagResource/UntagResource handlers and their tag state must publish into the shared tagging index resourcegroupstaggingapi GetResources reads (and GetResources must resolve the lambda:function resource type against it), rather than keeping a Lambda-private tag map. Related open issues report the same missing-index symptom for other resource types: 20261024T043150Z-resource-groups-tagging-api-getresources-never-answers-for.md and 20261024T043741Z-apigateway-lambda-logs-resource-groups-t-apigateway-lambda-logs-resource-groups-tagging-api-sqs.md; neither covers lambda:function.
Traces: lambda__resource-groups-tagging-api.blueprint-tag-applied-after-create-surfaces-in-the-tag-query, lambda__resource-groups-tagging-api.blueprint-tag-untag-removes-the-function-from-the-tag-query.

Cause 2 of 2: a CloudFormation stack declaring AWS::Lambda::Function reaches completion but the function does not exist.
AWS: when a stack containing two AWS::Lambda::Function resources completes, both functions exist and are readable, each carrying the template's Tags plus CloudFormation's three aws:cloudformation:* system tags; lambda ListTags on the published function ARN returns 200 with those tags. ResourceNotFoundException (Smithy: lambda#ListTags error ResourceNotFoundException, HTTP 404) is reserved for a function that does not exist.
Emulator: immediately after the deploy phase, lambda ListTags on the stack's published tagged-function ARN answers ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account>:function:<name>" — the stack reported its resources created but no Lambda function was provisioned, so the tag assertions (template tags + the three aws:cloudformation:* system tags) can never be reached.
Why: any IaC-driven workflow is unusable for Lambda: the stack claims success and every subsequent read of the function fails as not-found, which a client cannot distinguish from a deleted resource.
Fix: CloudFormation resource provisioning for AWS::Lambda::Function must actually create the function (and propagate the template Tags plus the aws:cloudformation:stack-name/logical-id/stack-id system tags) before the resource is reported created. Open issues describe the same CREATE_COMPLETE-without-provisioning pattern for other resource types (20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md, 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md); neither covers AWS::Lambda::Function.
Traces: lambda__resource-groups-tagging-api.blueprint-tag-e2e-find-tagged-functions-lifecycle (error in phase 1/4 deploy).

## Evidence
Cause 1 — Expected: after creating one function with Tags={"lambda-console:blueprint":"true"} and tagging a second with lambda TagResource, rgt.get_resources(TagFilters=[{Key:"lambda-console:blueprint"}], ResourceTypeFilters=["lambda:function"]) selects both ARNs with their tags, and after lambda UntagResource of that key the function leaves the query while remaining indexed under its other key ("other").
Actual: the query never returns either function; the bounded poll ("the create-time control is indexed" / "both functions indexed") never converges and both checks end with "Check timed out after 60.0s".

Cause 2 — Expected: after the stack deploys, lam.list_tags(Resource=<tagged function ARN>) returns 200 with lambda-console:blueprint=true plus exactly the three aws:cloudformation:* system tags.
Actual: botocore ClientError ResourceNotFoundException on ListTags — "[phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListTags operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-scn-e2ebp-tagged-<id>" — the deployed stack published an ARN for a function that was never created.

## Steps to Reproduce
Cause 1:
1. CreateFunction A with Tags={"lambda-console:blueprint":"true"}; CreateFunction B untagged; wait for Active.
2. Call resourcegroupstaggingapi GetResources with TagFilters=[{Key:"lambda-console:blueprint"}] and ResourceTypeFilters=["lambda:function"] — observe no mappings.
3. lambda TagResource on B with the same key, re-query — still no mappings. Likewise lambda UntagResource never changes the (empty) result.

Cause 2:
1. Deploy a CloudFormation stack declaring two AWS::Lambda::Function resources (one with a Tags list) and wait for the stack to complete.
2. Call lambda ListTags on the function ARN the stack outputs.
3. Observe ResourceNotFoundException "Function not found" instead of the function's tags.
