Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-component-resource-policy-workflow
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-component-resource-policy-workflow imagebuilder-create-component-supported-os-versions-roundtrip imagebuilder-image-recipe-component-list-roundtrip imagebuilder-image-recipe-lifecycle-with-policy
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateComponent|2xx->2xx|$.latestVersionReferences
Issue-Key: imagebuilder|GetComponent|2xx->2xx|$.component.billable
Issue-Key: imagebuilder|GetComponent|2xx->2xx|$.component.tags
Issue-Key: imagebuilder|GetComponent|2xx->2xx|$.latestVersionReferences
Issue-Key: imagebuilder|GetImageRecipe|2xx->2xx|$.imageRecipe.tags
Issue-Key: imagebuilder|GetImageRecipe|2xx->2xx|$.latestVersionReferences
Issue-Key: imagebuilder|CreateImageRecipe|2xx->2xx|$.latestVersionReferences
Issue-Key: imagebuilder|GetImageRecipe|2xx->2xx|$.imageRecipe.components
Issue-Key: imagebuilder|GetImageRecipe|2xx->2xx|$.imageRecipe.platform
Issue-Key: imagebuilder|ListImageRecipes|2xx->2xx|$.imageRecipeSummaryList[arn="arn
Issue-Commit: 6fdfa0adfee5efefb12453212234969d8b9de734

# Image Builder component and image-recipe responses omit latestVersionReferences and stored resource fields

> _Note: this issue was identified against the code base at commit `6fdfa0adfee5efefb12453212234969d8b9de734`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImageRecipe

## Description
SIX causes, all in the imagebuilder emulator's component / image-recipe serializers and state. Every one of them is a modeled output member that AWS populates on a 200 response and the emulator leaves out entirely, so the HTTP status matches but the body is short of fields clients read. All six must be closed before this issue is submitted.

Cause 1 of 6: CreateComponent omits latestVersionReferences.
AWS: Smithy: CreateComponentResponse members requestId, clientToken, componentBuildVersionArn, latestVersionReferences -> LatestVersionReferences{latestVersionArn, latestMajorVersionArn, latestMinorVersionArn, latestPatchVersionArn}, each an ImageBuilderArn. AWS returns the four wildcard-version ARNs for the component just created: the same ARN as the created build version with the version triple replaced by x.x.x, <major>.x.x, <major>.<minor>.x and the exact <major>.<minor>.<patch>. The resource name, account, region and version in those ARNs come from the request and the created resource; they are not fixed text.
Emulator: the whole latestVersionReferences object is absent from the 200 body ($.latestVersionReferences: expected an object with the four ARN members, got None).
Why: a client that resolves "latest" wildcard ARNs from a create response (the documented way to reference the newest major/minor/patch of a component) gets null and cannot build any follow-up request.
Fix: CreateComponent's response serializer in the imagebuilder handler must emit latestVersionReferences, derived from the created resource's ARN and semantic version.
Traces: imagebuilder-component-resource-policy-workflow#setup#1 (CreateComponent, 200->200).

Cause 2 of 6: GetComponent omits component.tags, component.billable and latestVersionReferences.
AWS: Smithy: GetComponentResponse{requestId, component -> Component, latestVersionReferences}. Component.tags is a TagMap and is returned as an empty map when the component carries no tags, rather than being omitted. Component.billable is recorded behavior only (the member is not in the on-disk model) and the recording shows it present as a boolean, false for a non-Marketplace component. latestVersionReferences carries the same four wildcard ARNs as in cause 1.
Emulator: all three are missing from the 200 body ($.component.tags expected {}, got None; $.component.billable expected false, got None; $.latestVersionReferences expected the four-ARN object, got None).
Why: callers that iterate component.tags or branch on billable dereference null; callers that follow latestVersionReferences lose the wildcard ARNs.
Fix: GetComponent's response serializer plus the component record in imagebuilder state (tags must round-trip as an empty map, billable must be stored/defaulted).
Traces: imagebuilder-create-component-supported-os-versions-roundtrip#task#2 (GetComponent, 200->200).

Cause 3 of 6: GetImageRecipe omits imageRecipe.tags and latestVersionReferences.
AWS: Smithy: GetImageRecipeResponse{requestId, imageRecipe -> ImageRecipe, latestVersionReferences}. ImageRecipe.tags is a TagMap returned as an empty map for an untagged recipe; latestVersionReferences holds the recipe's x.x.x / <major>.x.x / <major>.<minor>.x / exact-version ARNs, built from the recipe ARN in the request.
Emulator: both missing on the 200 ($.imageRecipe.tags expected {}, got None; $.latestVersionReferences expected the four-ARN object, got None).
Why: same class of null-dereference and lost wildcard ARNs as cause 2, on the recipe read path.
Fix: GetImageRecipe's response serializer and the image-recipe record in imagebuilder state.
Traces: imagebuilder-image-recipe-component-list-roundtrip#task#3 (GetImageRecipe, 200->200).

Cause 4 of 6: CreateImageRecipe omits latestVersionReferences.
AWS: Smithy: CreateImageRecipeResponse{requestId, clientToken, imageRecipeArn, latestVersionReferences}. The four ARNs are derived from the recipe that was just created, exactly as in cause 1.
Emulator: latestVersionReferences absent from the 200 body (expected the four-ARN object, got None).
Why: a create-then-reference flow has no wildcard ARN to use.
Fix: CreateImageRecipe's response serializer in the imagebuilder handler.
Traces: imagebuilder-image-recipe-lifecycle-with-policy#setup#1 (CreateImageRecipe, 200->200).

Cause 5 of 6: GetImageRecipe omits imageRecipe.platform and imageRecipe.components (and latestVersionReferences) for a recipe created with no components.
AWS: Smithy: ImageRecipe.platform is a Platform enum and ImageRecipe.components is a ComponentConfigurationList. AWS derives platform from the recipe's parent image and returns it on every read ('Linux' for a Linux parent image), and returns components as an empty list when the recipe was created without component configurations rather than omitting it. latestVersionReferences as in cause 3.
Emulator: $.imageRecipe.platform expected the recipe's platform, got None; $.imageRecipe.components expected [], got None; $.latestVersionReferences expected the four-ARN object, got None.
Why: a client that selects build behavior by recipe platform, or that iterates components to render/modify a recipe, sees null instead of a platform and instead of an empty list.
Fix: imagebuilder state must persist/derive the recipe platform at CreateImageRecipe time and GetImageRecipe's serializer must emit platform and an empty components list.
Traces: imagebuilder-image-recipe-lifecycle-with-policy#task#2 (GetImageRecipe, 200->200).

Cause 6 of 6: ListImageRecipes summaries omit platform.
AWS: Smithy: ListImageRecipesResponse.imageRecipeSummaryList -> ImageRecipeSummary{arn, name, platform, owner, parentImage, dateCreated, tags}. The summary for a recipe carries the same platform value its detail view reports.
Emulator: the summary entry for the listed recipe has no platform member ($.imageRecipeSummaryList[arn=...].platform expected the recipe's platform, got None).
Why: a client filtering or grouping recipes by platform from the list page gets null for every row and must fall back to a Get per recipe.
Fix: ListImageRecipes' summary projection in the imagebuilder handler, fed by the stored platform from cause 5.
Traces: imagebuilder-image-recipe-lifecycle-with-policy#task#3 (ListImageRecipes, 200->200).

Note: the only open imagebuilder issues are unimplemented-stub issues for CreateDistributionConfiguration and CreateImagePipeline and a CreateImagePipeline timeoutMinutes validation issue; none of them covers these response-shape gaps.

## Evidence
All six steps returned HTTP 200 where 200 was expected, so only the body diverged.
1) CreateComponent (imagebuilder-component-resource-policy-workflow#setup#1): expected $.latestVersionReferences = {latestVersionArn: <component ARN>/x.x.x, latestMajorVersionArn: .../1.x.x, latestMinorVersionArn: .../1.0.x, latestPatchVersionArn: .../1.0.0}; got None.
2) GetComponent (imagebuilder-create-component-supported-os-versions-roundtrip#task#2): $.component.tags expected {} got None; $.component.billable expected False got None; $.latestVersionReferences expected the four-ARN object got None.
3) GetImageRecipe (imagebuilder-image-recipe-component-list-roundtrip#task#3): $.imageRecipe.tags expected {} got None; $.latestVersionReferences expected the four wildcard image-recipe ARNs got None.
4) CreateImageRecipe (imagebuilder-image-recipe-lifecycle-with-policy#setup#1): $.latestVersionReferences expected the four wildcard image-recipe ARNs got None.
5) GetImageRecipe (imagebuilder-image-recipe-lifecycle-with-policy#task#2): $.imageRecipe.platform expected 'Linux' got None; $.imageRecipe.components expected [] got None; $.latestVersionReferences expected the four-ARN object got None.
6) ListImageRecipes (imagebuilder-image-recipe-lifecycle-with-policy#task#3): $.imageRecipeSummaryList[arn="...image-recipe/<name>/1.0.0"].platform expected 'Linux' got None.
(ARNs, resource names and version numbers above are that recording's values; the requirement is the member's presence and derivation, not the literal string.)

## Steps to Reproduce
1. Replay fixture imagebuilder-component-resource-policy-workflow step setup#1 (CreateComponent with a name, semanticVersion 1.0.0, platform Linux and inline data) and inspect the 200 body for latestVersionReferences.
2. Replay imagebuilder-create-component-supported-os-versions-roundtrip step task#2 (GetComponent on the created build-version ARN) and inspect component.tags, component.billable and latestVersionReferences.
3. Replay imagebuilder-image-recipe-component-list-roundtrip step task#3 (GetImageRecipe) and inspect imageRecipe.tags and latestVersionReferences.
4. Replay imagebuilder-image-recipe-lifecycle-with-policy steps setup#1 (CreateImageRecipe), task#2 (GetImageRecipe) and task#3 (ListImageRecipes) and inspect latestVersionReferences, imageRecipe.platform, imageRecipe.components and the summary's platform.

## Resolution

Fixed: `latestVersionReferences` on CreateComponent, GetComponent, CreateImageRecipe and
GetImageRecipe (causes 1, 2, 3 and 4), the resource `tags` map on every resource view
(causes 2 and 3), and the image recipe's `platform` and `components` on the read and in
the listed summary (causes 5 and 6).

Still outstanding, declined as outside the modelled contract: `$.component.billable`.
This service's `Component` shape declares no `billable` member
(`smithy_query(query="shape", shape="Component")`), and every response of this emulator is
serialized from the shape the model declares, so there is no member to populate. Beside it,
the replay of `imagebuilder-image-recipe-lifecycle-with-policy` now reaches its teardown and
diverges there on `$.requestId` of a 404 body — a cause this issue does not name, and the
same kind of gap: the modelled `ResourceNotFoundException` declares only `message`, and the
restJson1 error envelope is written by the framework.
