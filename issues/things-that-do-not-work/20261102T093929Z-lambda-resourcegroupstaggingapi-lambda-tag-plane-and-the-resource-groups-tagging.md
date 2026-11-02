Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__resource-groups-tagging-api.blueprint-tag-deleted-function-leaves-the-tag-query lambda__resource-groups-tagging-api.blueprint-tag-key-filter-is-case-sensitive lambda__resource-groups-tagging-api.blueprint-tag-key-filter-selects-only-the-tagged-function lambda__resource-groups-tagging-api.blueprint-tag-query-is-region-scoped lambda__resource-groups-tagging-api.blueprint-tag-query-paginates-one-function-per-page lambda__resource-groups-tagging-api.blueprint-tag-resource-type-filter-absence-widens-the-tag-query lambda__resource-groups-tagging-api.blueprint-tag-value-filter-matches-exact-strings-as-a-union lambda__resource-groups-tagging-api.blueprint-tag-versions-and-aliases-do-not-surface-as-tagged-resources lambda__resource-groups-tagging-api.blueprint-tag-keys-and-values-summaries-report-the-colon-key lambda__resource-groups-tagging-api.blueprint-tag-written-through-the-tagging-api-reads-back-through-lambda lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-system-key-rejected lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-missing-function-rejected-per-resource lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-version-arn-rejected-per-resource lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-fifty-one-tags-rejected lambda__resource-groups-tagging-api.blueprint-tag-e2e-find-tagged-functions-lifecycle
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-deleted-function-leaves-the-tag-query|_poll#6|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-key-filter-is-case-sensitive|_poll#3|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-key-filter-selects-only-the-tagged-function|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-query-is-region-scoped|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-query-paginates-one-function-per-page|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-resource-type-filter-absence-widens-the-tag-query|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-value-filter-matches-exact-strings-as-a-union|_poll#6|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-versions-and-aliases-do-not-surface-as-tagged-resources|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-keys-and-values-summaries-report-the-colon-key|_poll#6|Timeout|time.sleep(interval_s)
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-written-through-the-tagging-api-reads-back-through-lambda|run#8|AssertionError|assert lam.list_tags(Resource=arn)["Tags"] == {_KEY: "true"}, (
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-system-key-rejected|run#10|AssertionError|assert lam.list_tags(Resource=arn)["Tags"] == {_KEY: "true"}, "the control tag must land"
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-missing-function-rejected-per-resource|run#7|AssertionError|assert sorted(failed or {}) == [ghost], f"exactly the missing ARN must fail; got {failed!r}"
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-version-arn-rejected-per-resource|run#9|AssertionError|assert sorted(failed or {}) == [qualified], f"exactly the version ARN must fail; got {failed!r}"
Issue-Key: lambda__resource-groups-tagging-api.blueprint-tag-tagging-api-fifty-one-tags-rejected|run#9|AssertionError|assert (code, message) == ("ValidationException", expected_message), (
Issue-Key: lambda__resourcegroupstaggingapi|ListTags|ResourceNotFoundException
Issue-Commit: 9f5148798275000480c887daf4e17bbd7b05ffea

# Lambda tag plane and the Resource Groups Tagging index are disconnected, and TagResources refusals/per-resource failures diverge (7 causes)

> _Note: this issue was identified against the code base at commit `9f5148798275000480c887daf4e17bbd7b05ffea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
SEVEN distinct root causes, all surfaced by the lambda__resourcegroupstaggingapi behavioral checks. A fixer must close all seven before submitting; each cause below carries its own failing check ids.

Related OPEN issues that do NOT cover these: 20261024T113144Z-lambda-function-tags-are-absent-from-the-resource.md (it claims the index gap for the tag-after-create and untag paths and the CFN gap, but the checks below are a different, un-owned set and exercise GetTagKeys/GetTagValues, the RGT->Lambda write-through direction, FailedResourcesMap and the >50-tag message, none of which it names); 20261023T205437Z-resource-groups-tagging-api-tagresources-silently-succeeds-for.md (same empty-FailedResourcesMap symptom, scoped to a generic nonexistent ARN, not to Lambda's own 404 contract); 20261102T083644Z-resourcegroupstaggingapi-getresources-never-returns-resources-tagged-by-their-native.md; 20261029T194554Z-resourcegroupstaggingapi-tagresources-invalidparameterexception-message-for-a-non-arn-value.md.

=== Cause 1 of 7: tags on a lambda:function never appear in GetResources, so every tag-query semantic over Lambda is unobservable ===
AWS: tags supplied to CreateFunction's Tags, or added later with lambda TagResource (Smithy: lambda#TagResource POST /2017-03-31/tags/{Resource}, 204, Resource @pattern arn:...:lambda:<region>:<account>:function:<name>, Tags map), enter the same account/region tag store that resourcegroupstaggingapi#GetResources reads (POST /GetResources via X-Amz-Target, output ResourceTagMappingList of ResourceTagMapping{ResourceARN,Tags}, paginated by PaginationToken with pageSize ResourcesPerPage). Consequently: a key-only TagFilters entry selects exactly the functions carrying that exact key (prefix keys and keys appearing as a VALUE do not match); key and value matching is case-sensitive and a Values list is a union of exact strings; ResourcesPerPage caps each page and following PaginationToken enumerates every match exactly once; omitting ResourceTypeFilters (or passing it empty) widens the query across services while ResourceTypeFilters=["lambda:function"] narrows it; the index is per-region so another region's query never returns this region's ARNs; DeleteFunction withdraws the function from the query while an identically tagged sibling stays; and only the UNQUALIFIED function ARN is ever indexed (versions and aliases are not taggable resources). Recorded behavior shows the index trailing a write by about 3 s.
Emulator: GetResources returns no lambda:function mapping at all. Every one of these checks first polls a bounded window for the baseline condition "the tagged function is indexed" and that poll never converges, so each check is killed at the harness budget with "Check timed out after 60.0s". Lambda's tag state and the tagging index are two separate stores.
Why: a client cannot answer "which functions carry tag X" through the cross-service Tagging API — the normal path for tag-driven discovery, cost allocation, region sweeps and cleanup. It returns an empty list rather than an error, so the caller silently concludes no Lambda function is tagged, and none of the filter/pagination/region/deletion semantics it relies on can be exercised at all.
Fix: the Lambda CreateFunction/TagResource/UntagResource/DeleteFunction handlers and their tag state must publish into (and retract from) the shared tagging index that the resourcegroupstaggingapi GetResources handler reads, and that handler must resolve the lambda:function resource type against it — including per-region scoping and the rule that only unqualified function ARNs are indexed.
Traces: blueprint-tag-key-filter-selects-only-the-tagged-function, blueprint-tag-key-filter-is-case-sensitive, blueprint-tag-value-filter-matches-exact-strings-as-a-union, blueprint-tag-query-paginates-one-function-per-page, blueprint-tag-resource-type-filter-absence-widens-the-tag-query, blueprint-tag-query-is-region-scoped, blueprint-tag-deleted-function-leaves-the-tag-query, blueprint-tag-versions-and-aliases-do-not-surface-as-tagged-resources.

=== Cause 2 of 7: GetTagKeys/GetTagValues do not report a key/value written through Lambda ===
AWS: Smithy resourcegroupstaggingapi#GetTagKeys (output TagKeys, paginated) lists every tag key present in the account/region, and #GetTagValues (input Key @required @length 1..128, output TagValues, paginated) lists the distinct values recorded under it. A key containing a colon, e.g. lambda-console:blueprint, is ONE key: the colon is data, not a namespace separator, so no separate lambda-console key is ever produced, and a value is listed once however many resources carry it.
Emulator: after creating a function with Tags={"lambda-console:blueprint": <value>, ...}, the bounded poll on the GetTagValues(Key="lambda-console:blueprint") paginator never observes the value and the check is killed at 60 s. The key/value summaries are fed from the same disconnected index as Cause 1, so they are empty for Lambda-written tags.
Why: tag governance and policy tooling enumerate keys and values first to build a picture of the account; those endpoints report an account with no Lambda tag keys, so every downstream decision is taken on an empty inventory.
Fix: the GetTagKeys and GetTagValues handlers must derive their key/value summaries from the shared tagging index that Cause 1's fix populates, treating the whole tag key (colon included) as one opaque key.
Traces: blueprint-tag-keys-and-values-summaries-report-the-colon-key.

=== Cause 3 of 7: a tag written through RGT TagResources does not reach Lambda's own tag store ===
AWS: Smithy resourcegroupstaggingapi#TagResources (ResourceARNList @required @length 1..20, Tags @required @length 1..50, output FailedResourcesMap) writes into the resource's own tag set, so a successful call naming a function ARN is immediately readable back through lambda#ListTags (GET /2017-03-31/tags/{Resource}, 200, output Tags map) — the write originates at the Tagging API and the read is Lambda's store; they are one store.
Emulator: TagResources answers 200 with an empty FailedResourcesMap (so the call reports success), but lambda ListTags on the same unqualified function ARN then returns Tags={}. Observed as "AssertionError: Lambda ListTags must see the tag; got {}" and, in the system-key check, "AssertionError: the control tag must land" on the control write that follows the aws:-key refusal.
Why: the write-through direction is silently lost. A client that tags via the Tagging API and reads via the service API — the normal split between a governance tool and an application — sees its tag vanish with no error to detect it, and any read-modify-write on the function's tags will drop it.
Fix: the resourcegroupstaggingapi TagResources/UntagResources handlers must apply the mutation to the owning service's tag state (here the Lambda function tag map) rather than to a tagging-API-private map.
Traces: blueprint-tag-written-through-the-tagging-api-reads-back-through-lambda, blueprint-tag-tagging-api-system-key-rejected.

=== Cause 4 of 7: a nonexistent function ARN in a TagResources batch is not reported in FailedResourcesMap ===
AWS: TagResources is per-resource: it answers HTTP 200 and places every ARN it could not tag in FailedResourcesMap (map ResourceARN -> FailureInfo{StatusCode,ErrorCode,ErrorMessage}) carrying the OWNING service's refusal. For a Lambda function ARN that does not exist the entry is StatusCode 404, ErrorCode "ResourceNotFoundException", ErrorMessage "Function not found: <arn>" — fixed text "Function not found: " followed by the request's ARN verbatim (Smithy: lambda#ListTags/TagResource error ResourceNotFoundException, httpError 404); the live ARN in the same batch IS tagged.
Emulator: the batch [live ARN, absent ARN] returns FailedResourcesMap == {} — the absent ARN is reported as successfully tagged. Observed: "AssertionError: exactly the missing ARN must fail; got {}".
Why: a per-resource API that never fails a resource makes partial failure undetectable; a client batching tag writes over an inventory that has drifted believes every resource was tagged and never retries or reports the missing one.
Fix: the TagResources handler must resolve each ARN against the owning service's state and emit a FailureInfo entry with that service's status/code/message for every ARN it could not tag, while still tagging the resolvable ones.
Traces: blueprint-tag-tagging-api-missing-function-rejected-per-resource.

=== Cause 5 of 7: a version-qualified Lambda ARN is accepted by TagResources instead of failing per resource ===
AWS: Lambda tags belong to the function, never to a version or alias, so a qualified ARN (<function-arn>:<version>) is refused. Through TagResources that refusal is a per-resource FailedResourcesMap entry with StatusCode 400, ErrorCode "InvalidParameterValueException" (Smithy: lambda#TagResource error InvalidParameterValueException, httpError 400) and the fixed ErrorMessage "Tags on function aliases and versions are not supported. Please specify a function ARN." — this message has no variable parts. The unqualified ARN in the same batch is tagged.
Emulator: the batch [unqualified ARN, "<arn>:1"] returns FailedResourcesMap == {}; the version ARN is accepted. Observed: "AssertionError: exactly the version ARN must fail; got {}".
Why: tags appear to have been attached to an immutable version, which cannot hold them. The caller's tag inventory records a resource identity AWS never tags, and the real function may be left untagged while the call reports total success.
Fix: TagResources' per-resource resolution of lambda ARNs must reject a qualified (version or alias) ARN with that 400 InvalidParameterValueException FailureInfo entry instead of treating it as a taggable resource.
Traces: blueprint-tag-tagging-api-version-arn-rejected-per-resource.

=== Cause 6 of 7: the >50-tag ValidationException message renders a count instead of the rejected tag map ===
AWS: Tags on TagResources is @length min 1 max 50 (Smithy: resourcegroupstaggingapi#TagResourcesInput.Tags, target TagMap @length 1..50), and 51 tags are refused whole-call with no tag written. The refusal is ValidationException and its message is AWS's standard constraint sentence, which ECHOES THE REJECTED VALUE: "1 validation error detected: Value '<rendered tags map>' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50". Fixed text: "1 validation error detected: Value '", "' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"; variable part: the rendered map of the request's own tags, in the form {k=v, k=v, ...} (the member name 'tags' and the limit 50 come from the model).
Emulator: same code, wrong message — it substitutes a COUNT for the value: got ("ValidationException", "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"). The literal "51 entries" placeholder is the divergence; the rejected value itself is never rendered.
Why: clients and SDK error-parsing/logging surfaces key off the echoed value to show the operator WHICH tags were rejected; a count placeholder removes that, and any assertion or log scrape expecting AWS's constraint sentence does not match.
Fix: the constraint-violation message formatter behind TagResources' Tags length validation must render the offending collection value, not a "<n> entries" summary.
Traces: blueprint-tag-tagging-api-fifty-one-tags-rejected.

=== Cause 7 of 7: a CloudFormation stack with AWS::Lambda::Function completes but the function does not exist ===
AWS: when a stack declaring AWS::Lambda::Function resources completes, each function exists and lambda#ListTags on its ARN answers 200 with the template's tags plus CloudFormation's aws:cloudformation:* system tags. ResourceNotFoundException (httpError 404) is reserved for a function that does not exist.
Emulator: immediately after the deploy phase the check's ListTags on the stack's published function ARN raises ResourceNotFoundException "Function not found: <arn>" — fixed text "Function not found: " plus the requested ARN — so the stack reported its resources created without provisioning the function, and the check aborts in phase 1/4 (deploy) before any tag assertion. Open issue 20261024T113144Z-lambda-function-tags-are-absent-from-the-resource.md describes this same CREATE_COMPLETE-without-provisioning pattern as its Cause 2 but does not own this check's replay key; it is recorded here so the check is not lost.
Why: IaC-driven Lambda is unusable — the stack claims success and every subsequent read of the function fails as not-found, indistinguishable to a client from a deleted resource.
Fix: CloudFormation resource provisioning for AWS::Lambda::Function must actually create the function, and propagate the template Tags plus the aws:cloudformation:stack-name/logical-id/stack-id system tags, before reporting the resource created.
Traces: blueprint-tag-e2e-find-tagged-functions-lifecycle (error in phase 1/4 deploy).

## Evidence
Cause 1 — Expected: after CreateFunction with Tags={"lambda-console:blueprint":"true"} (and the sibling/decoy functions each check builds), rgt.get_resources(TagFilters=[{Key:"lambda-console:blueprint"}], ResourceTypeFilters=["lambda:function"]) selects exactly the matching function ARNs with their tag pairs, honouring exact-key matching, case-sensitive value unions, ResourcesPerPage/PaginationToken walks, the widening effect of omitting (or emptying) ResourceTypeFilters, per-region scoping and withdrawal on DeleteFunction.
Actual: the query returns no lambda:function mapping; the baseline bounded poll ("<arn> indexed", "both functions indexed", "all three functions indexed", "function and queue both indexed under the shared key") never converges and all eight checks end with "Error: Check timed out after 60.0s".

Cause 2 — Expected: rgt GetTagValues(Key="lambda-console:blueprint") lists the function's value once and GetTagKeys lists "lambda-console:blueprint" (and no "lambda-console" key).
Actual: the poll "GetTagValues('lambda-console:blueprint') lists '<value>'" never converges; "Error: Check timed out after 60.0s".

Cause 3 — Expected: rgt.tag_resources(ResourceARNList=[<function arn>], Tags={"lambda-console:blueprint":"true"}) -> 200, FailedResourcesMap {}, then lam.list_tags(Resource=<function arn>)["Tags"] == {"lambda-console:blueprint":"true"}.
Actual: TagResources answers 200 with FailedResourcesMap {}, then ListTags returns {} — "AssertionError: Lambda ListTags must see the tag; got {}" and "AssertionError: the control tag must land".

Cause 4 — Expected: rgt.tag_resources(ResourceARNList=[<live function arn>, <absent function arn>], Tags={...}) -> 200 with FailedResourcesMap containing ONLY the absent ARN, as {StatusCode: 404, ErrorCode: "ResourceNotFoundException", ErrorMessage: "Function not found: <absent arn>"}, and the live ARN tagged.
Actual: FailedResourcesMap == {} — "AssertionError: exactly the missing ARN must fail; got {}".

Cause 5 — Expected: rgt.tag_resources(ResourceARNList=[<function arn>, <function arn>:1], Tags={...}) -> 200 with FailedResourcesMap containing ONLY the version ARN, as {StatusCode: 400, ErrorCode: "InvalidParameterValueException", ErrorMessage: "Tags on function aliases and versions are not supported. Please specify a function ARN."}.
Actual: FailedResourcesMap == {} — "AssertionError: exactly the version ARN must fail; got {}".

Cause 6 — Expected: rgt.tag_resources with 51 tags -> ValidationException, message "1 validation error detected: Value '{<k>=<v>, ...all 51 pairs...}' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", and nothing written.
Actual: ("ValidationException", "1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50") — the rejected value is replaced by the literal placeholder "51 entries".

Cause 7 — Expected: after the stack completes, lam.list_tags(Resource=<stack's function arn>) -> 200 with the template tag plus the three aws:cloudformation:* system tags.
Actual: "[phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListTags operation: Function not found: arn:aws:lambda:<region>:<account>:function:<name>".

## Steps to Reproduce
Cause 1:
1. iam CreateRole with the lambda.amazonaws.com trust policy; lambda CreateFunction (python3.12, zip payload) with Tags={"lambda-console:blueprint":"true"}; wait for Active.
2. Call resourcegroupstaggingapi GetResources with TagFilters=[{"Key":"lambda-console:blueprint"}] and ResourceTypeFilters=["lambda:function"]. Observe ResourceTagMappingList == [] and that it stays empty however long you poll.
3. Repeat with decoys (a function tagged with a longer key that this key prefixes, a function whose tag VALUE spells the key), with Values=["true"] / ["True"] / ["false","true"], with ResourcesPerPage=1 over three matches, with ResourceTypeFilters omitted/empty alongside an SQS queue tagged with the same key, against a second region, and after DeleteFunction — none can be observed because step 2's baseline never holds.

Cause 2:
1. CreateFunction with Tags={"lambda-console:blueprint": "<value>", "sim2real:managed":"true"}.
2. Page resourcegroupstaggingapi GetTagValues(Key="lambda-console:blueprint") and GetTagKeys. Observe the value and key are never listed.

Cause 3:
1. CreateFunction with no tags; wait for Active.
2. resourcegroupstaggingapi TagResources(ResourceARNList=[<function arn>], Tags={"lambda-console:blueprint":"true"}) — observe 200, FailedResourcesMap {}.
3. lambda ListTags(Resource=<function arn>) — observe {} instead of the tag.

Cause 4:
1. CreateFunction and wait for Active; build a second ARN in the same account/region naming a function that does not exist.
2. resourcegroupstaggingapi TagResources(ResourceARNList=[<live arn>, <absent arn>], Tags={"lambda-console:blueprint":"true"}).
3. Observe FailedResourcesMap == {} instead of one 404 ResourceNotFoundException entry keyed by the absent ARN.

Cause 5:
1. CreateFunction, wait for Active, lambda PublishVersion -> "<function arn>:1".
2. resourcegroupstaggingapi TagResources(ResourceARNList=[<function arn>, <function arn>:1], Tags={"lambda-console:blueprint":"true"}).
3. Observe FailedResourcesMap == {} instead of one 400 InvalidParameterValueException entry keyed by the version ARN.

Cause 6:
1. CreateFunction and wait for Active.
2. resourcegroupstaggingapi TagResources(ResourceARNList=[<function arn>], Tags={f"k{i}":"v" for i in range(51)}).
3. Read Error.Message — observe "Value '51 entries'" where AWS echoes the rendered 51-pair tag map.

Cause 7:
1. cloudformation CreateStack with a template declaring two AWS::Lambda::Function resources (one carrying Tags) and wait for the stack to complete.
2. lambda ListTags on the function ARN the stack outputs.
3. Observe ResourceNotFoundException "Function not found: <arn>" instead of the function's tags.
