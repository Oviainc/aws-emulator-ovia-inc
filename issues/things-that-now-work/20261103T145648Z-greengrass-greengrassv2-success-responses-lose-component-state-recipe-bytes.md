Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-component-version-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-component-version-lifecycle greengrassv2-component-error-paths greengrassv2-deployment-tag-round-trip
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|DescribeComponent|2xx->2xx|$.status.componentState
Issue-Key: greengrass|CreateComponentVersion|4xx->4xx|$.message
Issue-Key: greengrass|GetComponent|2xx->2xx|$.recipe
Issue-Key: greengrass|ListTagsForResource|2xx->2xx|$.tags.env
Issue-Key: greengrass|ListTagsForResource|2xx->2xx|$.tags.owner
Issue-Key: greengrass|ListTagsForResource|2xx->2xx|$.tags.sim2real-key
Issue-Key: greengrass|GetDeployment|2xx->2xx|$.components
Issue-Key: greengrass|GetDeployment|2xx->2xx|$.tags.sim2real-key
Issue-Commit: 66471a963a64e2b8a9116c3903f472052675d9f3

# greengrassv2 success responses lose component state, recipe bytes and tags

> _Note: this issue was identified against the code base at commit `66471a963a64e2b8a9116c3903f472052675d9f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComponent

## Description
The implemented greengrassv2 read paths answer HTTP 200 but drop state the service persists: a created component version never leaves the REQUESTED state, GetComponent does not return the recipe document AWS stores, and tags attached to component versions and deployments are never reported back. 7 causes to close.

Related-but-not-covering open issues (none carries these wire replay keys; both are behavioral-corpus findings): 20261103T065241Z-greengrassv2-refusal-responses-diverge-from-aws-10-net.md (its Cause 8 is about the duplicate-conflict SENTENCE, which the emulator now emits; Cause 2 here is the componentState VALUE inside it) and 20261103T065241Z-greengrassv2-tagresource-stub.md (TagResource is a separate stub item; Causes 4-7 here are the tag values that ListTagsForResource/GetDeployment must report once tags are stored).

Cause 1 of 7: a created component version never settles into the DEPLOYABLE state. AWS: Smithy: DescribeComponent GET /greengrass/v2/components/{arn}/metadata returns `status` (CloudComponentStatus) whose `componentState` is a CloudComponentState enum with values REQUESTED, INITIATED, DEPLOYABLE, FAILED, DEPRECATED; CreateComponentVersion returns status with componentState REQUESTED. Recorded behavior: after a successful CreateComponentVersion the service processes the recipe and the version becomes `DEPLOYABLE` — DescribeComponent on the new version reports status.componentState = 'DEPLOYABLE'. Emulator: DescribeComponent keeps reporting status.componentState = 'REQUESTED' indefinitely; there is no transition at all. Why: DEPLOYABLE is the gate every client waits on before deploying a component, so any wait-until-deployable loop written against AWS never terminates, and code that refuses to deploy a non-DEPLOYABLE version can never deploy anything on the emulator. Fix: the component-version state machine in the component store — CreateComponentVersion must record REQUESTED and the version must advance to DEPLOYABLE (its terminal success state) so DescribeComponent/GetComponent/ListComponents report it.

Cause 2 of 7: the duplicate-version ConflictException reports the stuck state, so its message is wrong. AWS: Smithy: CreateComponentVersion models ConflictException (httpError 409, "Your request has conflicting operations."). Recorded behavior: re-creating an existing component version is refused 409 with `Component [<componentName> : <componentVersion>] for account [<accountId>] already exists with state: [<componentState>]` — fixed text `Component [`, ` : `, `] for account [`, `] already exists with state: [`, `]`; the variables are the name and version from the recipe, the caller's account id, and the STORED component state, which for a settled version is DEPLOYABLE. Emulator: the same sentence but with state `[REQUESTED]`, because of Cause 1. Why: the message is a requirement and the state it carries is the one piece of state the caller learns about the winning version; a client told REQUESTED concludes the existing version is still being processed. Fix: follows from Cause 1 (the stored state) — CreateComponentVersion's duplicate-detection refusal must report the component's real state.

Cause 3 of 7: GetComponent does not return the recipe document AWS stores. AWS: Smithy: GetComponent GET /greengrass/v2/components/{arn} returns required `recipe` (RecipeBlob) and required `recipeOutputFormat`. Recorded behavior: for recipeOutputFormat JSON the service returns the component's recipe as a COMPACT JSON document (no space after `:` or `,`) that includes the service-assigned `ComponentType` member (`aws.greengrass.generic` for a generic component) and preserves the recipe's empty members — the per-manifest `Artifacts` list and the top-level `Lifecycle` object. Emulator: returns a re-serialized document with a space after every `:` and `,`, with `ComponentType` absent, and with the manifest's `Artifacts` and the top-level `Lifecycle` members dropped. Why: the recipe is a blob a client parses, diffs or re-submits; the byte difference breaks checksum/round-trip comparisons, and the missing ComponentType and empty Artifacts/Lifecycle members mean a client that reads recipe['ComponentType'] or iterates Manifests[i]['Artifacts'] raises a KeyError against the emulator where AWS supplies them. Fix: the component store must retain the normalized recipe document produced at CreateComponentVersion time (adding ComponentType and keeping the submitted empty members) and GetComponent's serializer must return those bytes rather than re-encoding the parsed recipe.

Cause 4 of 7: tags supplied at CreateComponentVersion are not reported by ListTagsForResource. AWS: Smithy: CreateComponentVersion takes `tags` (TagMap, 1..200); ListTagsForResource GET /tags/{resourceArn} returns `tags` (TagMap). Recorded behavior: a component version created with tags answers ListTagsForResource on its component-version ARN with exactly those key/value pairs. Emulator: 200 with no tag entries — every requested key is absent from `$.tags`. Why: the create-with-tags round trip is broken, so tag-based inventory, cost allocation and any read-after-write assertion on tags sees an untagged resource. Fix: CreateComponentVersion's handler must persist the request's tags into the shared tag store keyed by the component-version ARN, and ListTagsForResource must read them from it.

Cause 5 of 7: the same loss after a tag mutation leaves the surviving tags unreported. AWS: as Cause 4 — after tags are added/removed on a component version, ListTagsForResource returns the remaining key/value pairs. Recorded behavior: the step expects the surviving tag key to still be present with its value. Emulator: 200 with that key absent from `$.tags`. Why: a client cannot verify the result of a tag mutation; the resource reads as untagged whatever it does. Fix: the same tag store — the mutation paths (TagResource/UntagResource) and ListTagsForResource must share one keyed-by-ARN map.

Cause 6 of 7: tags on a DEPLOYMENT are not reported by ListTagsForResource either. AWS: Smithy: CreateDeployment takes `tags` (TagMap, 1..200) and ListTagsForResource accepts a GenericV2ARN whose resource segment may be `deployments`. Recorded behavior: a deployment created with tags answers ListTagsForResource on its deployment ARN with those key/value pairs. Emulator: 200 with the key absent from `$.tags`. Why: the deployment tagging surface round-trips to nothing, so tag-scoped deployment queries and read-after-write checks fail. Fix: CreateDeployment's handler must persist the request's tags for the deployment ARN into the same tag store ListTagsForResource reads.

Cause 7 of 7: GetDeployment omits both `tags` and `components` from its response. AWS: Smithy: GetDeployment output carries `components` (ComponentDeploymentSpecifications map) and `tags` (TagMap). Recorded behavior: GetDeployment on a tagged deployment returns the deployment's tags, and returns the `components` member as an empty map `{}` when the deployment specifies no components. Emulator: neither member is present in the 200 body. Why: a client reading response['components'] to enumerate a deployment's component specifications, or response['tags'] to read its tags, raises a KeyError instead of seeing AWS's map; the tags written at create time are unreadable through the deployment itself as well as through the tagging surface. Fix: GetDeployment's handler/serializer must emit `components` (empty map when none are specified) and populate `tags` from the deployment's stored tags.

## Evidence
Per-cause expected (AWS, from the recorded response) vs actual (emulator), quoted from the replay output. Component names, versions, account ids and ARNs are per-run values from the recording, not requirements.

Cause 1 — greengrassv2-component-version-lifecycle#task#2, DescribeComponent (status 200 on both sides):
  $.status.componentState expected 'DEPLOYABLE', got 'REQUESTED'

Cause 2 — greengrassv2-component-error-paths#task#2, CreateComponentVersion (status 409 on both sides):
  $.message expected 'Component [<name> : 1.0.0] for account [<acct>] already exists with state: [DEPLOYABLE]'
  $.message actual   'Component [<name> : 1.0.0] for account [<acct>] already exists with state: [REQUESTED]'
  (recorded run: sim2real-probe-greengrassv2-dup-comp-0dcd103b : 1.0.0, account 000000000000 — only the bracketed state diverges)

Cause 3 — greengrassv2-component-version-lifecycle#task#3, GetComponent (status 200 on both sides), $.recipe base64 decoded:
  expected {"RecipeFormatVersion":"2020-01-25","ComponentName":"<name>","ComponentVersion":"1.0.0","ComponentType":"aws.greengrass.generic","ComponentDescription":"Probe test component","ComponentPublisher":"sim2real-probe","ComponentConfiguration":{"DefaultConfiguration":{}},"Manifests":[{"Platform":{"os":"all"},"Lifecycle":{},"Artifacts":[]}],"Lifecycle":{}}
  actual   {"RecipeFormatVersion": "2020-01-25", "ComponentName": "<name>", "ComponentVersion": "1.0.0", "ComponentDescription": "Probe test component", "ComponentPublisher": "sim2real-probe", "ComponentConfiguration": {"DefaultConfiguration": {}}, "Manifests": [{"Platform": {"os": "all"}, "Lifecycle": {}}]}
  differences: spaces after ':' and ',' where AWS is compact; 'ComponentType':'aws.greengrass.generic' missing; the manifest's 'Artifacts':[] missing; the top-level 'Lifecycle':{} missing

Cause 4 — greengrassv2-component-version-lifecycle#task#7, ListTagsForResource (status 200 on both sides):
  $.tags.owner expected 'sim2real', got None (absent)
  $.tags.env   expected 'probe',    got None (absent)

Cause 5 — greengrassv2-component-version-lifecycle#task#9, ListTagsForResource (status 200 on both sides):
  $.tags.env expected 'probe', got None (absent)

Cause 6 — greengrassv2-deployment-tag-round-trip#task#5, ListTagsForResource (status 200 on both sides):
  $.tags.sim2real-key expected 'sim2real-value', got None (absent)

Cause 7 — greengrassv2-deployment-tag-round-trip#task#6, GetDeployment (status 200 on both sides):
  $.components        expected {} (empty map present), got None (member absent)
  $.tags.sim2real-key expected 'sim2real-value', got None (absent)

## Steps to Reproduce
Boot the emulator and point a greengrassv2 client at it. <acct> is the caller's account id, <region> its region.
1. Causes 1 and 3-5 (fixture greengrassv2-component-version-lifecycle, steps task#2, task#3, task#7, task#9): create_component_version(inlineRecipe=<minimal 2020-01-25 JSON recipe for NAME:1.0.0 with an os=all manifest, empty Artifacts and empty Lifecycle>, tags={'owner':'sim2real','env':'probe'}); then describe_component(arn=<returned arn>) — expect status.componentState 'DEPLOYABLE', observe 'REQUESTED'; get_component(arn=<returned arn>, recipeOutputFormat='JSON') — expect the compact recipe document including ComponentType 'aws.greengrass.generic' and the empty Artifacts/Lifecycle members, observe the spaced re-serialization with those members missing; list_tags_for_resource(resourceArn=<returned arn>) — expect {'owner':'sim2real','env':'probe'}, observe no tags; then mutate the tag set and list again — expect the surviving key, observe no tags.
2. Cause 2 (greengrassv2-component-error-paths, task#2): call create_component_version twice with the same recipe for NAME:1.0.0 — the second call must be refused 409 with '... already exists with state: [DEPLOYABLE]'; observe '[REQUESTED]'.
3. Causes 6-7 (greengrassv2-deployment-tag-round-trip, task#5 and task#6): create_deployment(targetArn='arn:aws:iot:<region>:<acct>:thing/<existing-thing>', tags={'sim2real-key':'sim2real-value'}); list_tags_for_resource(resourceArn='arn:aws:greengrass:<region>:<acct>:deployments:<deploymentId>') — expect that tag, observe none; get_deployment(deploymentId=<id>) — expect a 'components' member ({} when no components were specified) and the 'tags' map, observe both members absent.
