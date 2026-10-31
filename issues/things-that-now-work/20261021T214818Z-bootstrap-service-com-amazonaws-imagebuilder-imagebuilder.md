Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the imagebuilder emulator

_Filed: 2026-10-21T21:48:18Z_
_Operation: service-com-amazonaws-imagebuilder-imagebuilder | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 77 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and `src/state.rs` holds no EC2 Image Builder data. Nothing works end to end.

First slice (keep it this small): the component + image-recipe authoring lifecycle, which is the entry point of every real Image Builder workflow. Wire protocol is `restJson1`, signing name `imagebuilder`.

1. **CreateComponent** — `PUT /CreateComponent`. Required input: `name`, `semanticVersion` (`^[0-9]+\.[0-9]+\.[0-9]+$`), `platform` (Linux|Windows|macOS), `clientToken`, `dryRun` (default false); optional `description`, `changeDescription`, `supportedOsVersions`, `data` (YAML document), `uri`, `kmsKeyId`, `tags`. Must mint and PERSIST a component build version keyed by its generated ARN `arn:aws:imagebuilder:<region>:<account>:component/<name-lowercased-dashed>/<semanticVersion>/1`, and return `componentBuildVersionArn`, `clientToken` (echoed from input, as modeled) and a fresh `requestId`. Repeating the same `clientToken` with the same body must be idempotent (same ARN, no second record); same token with a different body must raise `IdempotentParameterMismatchException` (400). `data` and `uri` are mutually exclusive → `InvalidParameterCombinationException`; a malformed `semanticVersion` → `InvalidVersionNumberException`.
2. **GetComponent** — `GET /GetComponent?componentBuildVersionArn=...`. Must return the stored `component` (Component structure: `arn`, `name`, `version`, `description`, `changeDescription`, `type`, `platform`, `supportedOsVersions`, `state`, `owner`, `data`, `kmsKeyId`, `encrypted`, `dateCreated`, `tags`) reconstructed from the record written by CreateComponent — not a canned struct. Unknown ARN must fail (`ResourceNotFoundException`/`InvalidRequestException` per the model's declared errors for the op), never a hollow 200 with an empty component.
3. **CreateImageRecipe** — `PUT /CreateImageRecipe`. Required: `name`, `semanticVersion`, `parentImage`, `clientToken`, `dryRun`; optional `components` (list of ComponentConfiguration referencing the ARNs from step 1), `blockDeviceMappings`, `workingDirectory`, `additionalInstanceConfiguration`, `amiTags`, `tags`, `description`. Must persist the recipe and return the generated `imageRecipeArn` (`.../image-recipe/<name>/<semanticVersion>`). A `components` entry whose `componentArn` is not a stored component must be rejected with `InvalidRequestException`, proving the reference is validated against state. Creating the same name+version twice → `ResourceAlreadyExistsException`.
4. **GetImageRecipe** — `GET /GetImageRecipe?imageRecipeArn=...`. Must return the full stored `ImageRecipe` (`arn`, `name`, `description`, `platform` — inherited from the referenced components/parent image, `owner`, `version`, `components`, `parentImage`, `blockDeviceMappings`, `workingDirectory`, `dateCreated`, `tags`) exactly as created, including the component list in creation order.
5. **ListImageRecipes** — `POST /ListImageRecipes`. Must enumerate stored recipes as `imageRecipeSummaryList` (ImageRecipeSummary: `arn`, `name`, `platform`, `owner`, `parentImage`, `dateCreated`, `tags`), honour `owner` (`Self` returns the caller's recipes), `filters` (e.g. `name`), and paginate with `maxResults` (1..25) + `nextToken` — return a `nextToken` only when more items remain, and reject a token that was not issued with `InvalidPaginationTokenException`.
6. **DeleteImageRecipe** / **DeleteComponent** — `DELETE /DeleteImageRecipe?imageRecipeArn=...` and `DELETE /DeleteComponent?componentBuildVersionArn=...`. Must remove the record and echo the deleted ARN. Deleting a component that is still referenced by a stored recipe must fail with `ResourceDependencyException` (400); after deleting the recipe the component delete succeeds, and a subsequent Get/List no longer shows either resource.

No canned or echoed responses: every field in every response above must be computed from the persisted record (the only legitimately echoed value is `clientToken`, which the model defines as the request's idempotency token). Random ARNs that are not stored, hardcoded `Component`/`ImageRecipe` structs, or 200s with empty payloads all count as failures of this bootstrap.

## State Model

In `ComAmazonawsImagebuilderImagebuilderState` (src/state.rs), behind the engine lock:

- `components: BTreeMap<String /* componentBuildVersionArn */, ComponentRecord>` where `ComponentRecord { arn, name, version, build_version: u32, description, change_description, component_type: ComponentType (BUILD|TEST, derived from the YAML `phases`), platform, supported_os_versions: Vec<String>, state: ComponentState, owner: String /* account id */, data: Option<String>, uri: Option<String>, kms_key_id, encrypted: bool, date_created: String /* RFC3339 */, tags: BTreeMap<String,String> }`.
- `image_recipes: BTreeMap<String /* imageRecipeArn */, ImageRecipeRecord>` where `ImageRecipeRecord { arn, name, version, description, platform, owner, components: Vec<ComponentConfiguration>, parent_image, block_device_mappings, working_directory, additional_instance_configuration, ami_tags, tags, date_created }`.
- `idempotency: BTreeMap<(String /* operation */, String /* clientToken */), IdempotentResult { request_hash: u64, arn: String }>` so a replayed `clientToken` returns the first result and a mismatched body raises `IdempotentParameterMismatchException`.
- `pagination: BTreeMap<String /* nextToken */, PageCursor { op: String, last_key: String, filters_hash: u64 }>` so list tokens are server-issued and validated.
- Scalars: `account_id`, `region` (for ARN construction) and a monotonic `request_seq` used to mint each response's `requestId`.

Out of scope for this first slice (stay 501 and get filed as individual gap issues later): images and pipelines (CreateImage, CreateImagePipeline, StartImagePipelineExecution, GetImage, ListImages…), container recipes, infrastructure/distribution configurations, workflows, lifecycle policies, resource policies, tagging, and the scan/finding operations.

## Evidence

OPERATIONS.md (services/service-com-amazonaws-imagebuilder-imagebuilder/OPERATIONS.md) lists 77 operations with routes/members: CreateComponent (line 40, `PUT /CreateComponent`, required name/semanticVersion/platform/clientToken/dryRun, output componentBuildVersionArn), GetComponent (line 173, `GET /GetComponent`, output `component` of shape Component), CreateImageRecipe (line 75, output imageRecipeArn, errors include ResourceAlreadyExistsException/InvalidVersionNumberException), GetImageRecipe (line 229, output `imageRecipe`), ListImageRecipes (line 377, @paginated(inputToken=nextToken, outputToken=nextToken, items=imageRecipeSummaryList), maxResults range 1..25, InvalidPaginationTokenException), DeleteImageRecipe (line 138) and DeleteComponent (line 103, errors include ResourceDependencyException). Shape fields for the Component, ImageRecipe and ImageRecipeSummary structures above come from the Smithy model via smithy_query. The all-stub status is from `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` in this crate, which matches every operation file; no integration tests exist beyond the generated tests/state_roundtrip.rs.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `imagebuilder` client at it.
2. `aws imagebuilder create-component --name demo-component --semantic-version 1.0.0 --platform Linux --data "$(cat component.yaml)" --client-token tok-1` → expect 200 with a `componentBuildVersionArn`; today 501 NotImplemented.
3. Repeat the identical call with `--client-token tok-1` → expect the same ARN (idempotent), and with a changed `--description` → expect IdempotentParameterMismatchException (400).
4. `aws imagebuilder get-component --component-build-version-arn <arn from step 2>` → expect the component with the name, version, platform and data that were submitted.
5. `aws imagebuilder create-image-recipe --name demo-recipe --semantic-version 1.0.0 --parent-image ami-12345678 --components componentArn=<component arn> --client-token tok-2` → expect an `imageRecipeArn`; repeating it → ResourceAlreadyExistsException; using a bogus componentArn → InvalidRequestException.
6. `aws imagebuilder get-image-recipe --image-recipe-arn <recipe arn>` → expect components, parentImage, version and tags exactly as created.
7. `aws imagebuilder list-image-recipes --owner Self --max-results 1` → expect the recipe summary and a `nextToken` only when a second recipe exists; pass a fabricated nextToken → InvalidPaginationTokenException.
8. `aws imagebuilder delete-component --component-build-version-arn <component arn>` while the recipe still references it → expect ResourceDependencyException (400).
9. `aws imagebuilder delete-image-recipe --image-recipe-arn <recipe arn>` then retry the component delete → both succeed; a following get-image-recipe/get-component fails as not found and list-image-recipes returns an empty list.
