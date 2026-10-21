Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 543596051071af06ccfbb58d47ce453a1db8e676

# Bootstrap the ServerlessApplicationRepository emulator

_Filed: 2026-10-21T22:39:15Z_
_Operation: service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository | Class: missing_

> _Note: this issue was identified against the code base at commit `543596051071af06ccfbb58d47ce453a1db8e676`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 14 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` declares no fields at all (`// TODO: add service-specific state fields`). Nothing about this service works yet.

This issue scopes the FIRST buildable slice: the application lifecycle of the Serverless Application Repository, implemented statefully over the shared engine store so a realistic client script runs end-to-end. Six operations, in this order:

1. **CreateApplication** — `POST /applications -> 201`. Required input: `Name`, `Author`, `Description`; optional `HomePageUrl`, `Labels`, `LicenseUrl`/`LicenseBody`, `ReadmeUrl`/`ReadmeBody`, `SpdxLicenseId`, `SemanticVersion`, `TemplateBody`/`TemplateUrl`, `SourceCodeUrl`, `SourceCodeArchiveUrl`. Must mint and persist an `ApplicationId` (an ARN of the form `arn:aws:serverlessrepo:<region>:<account>:applications/<Name>`), record `CreationTime`, store every supplied field, and if `SemanticVersion` was given also create the initial version and return it as the nested `Version`. Output echoes the STORED record (`ApplicationId`, `Name`, `Author`, `Description`, `CreationTime`, `HomePageUrl`, `Labels`, `LicenseUrl`, `ReadmeUrl`, `SpdxLicenseId`, `IsVerifiedAuthor`, `Version`). A second CreateApplication with the same `Name` must fail `ConflictException` (409).
2. **GetApplication** — `GET /applications/{ApplicationId} -> 200`, optional `?semanticVersion=`. Must look the application up in the store and return the persisted record; when `SemanticVersion` is supplied, the returned `Version` must be THAT stored version, and an unknown version or unknown ApplicationId must be `NotFoundException` (404).
3. **CreateApplicationVersion** — `PUT /applications/{ApplicationId}/versions/{SemanticVersion} -> 201`. Input `TemplateBody`/`TemplateUrl`, `SourceCodeUrl`, `SourceCodeArchiveUrl`. Must append a new version to the application's version list, keyed by the `SemanticVersion` path label, and return `ApplicationId`, `SemanticVersion`, `CreationTime`, `TemplateUrl`, `ParameterDefinitions` (may be an empty list in the first slice), `RequiredCapabilities`, `ResourcesSupported`. Re-creating an existing semantic version must be `ConflictException` (409); unknown ApplicationId must be 404.
4. **ListApplicationVersions** — `GET /applications/{ApplicationId}/versions -> 200`. Must return one `VersionSummary` (`ApplicationId`, `SemanticVersion`, `CreationTime`, `SourceCodeUrl`) per version actually stored for that application, honouring `maxItems` (1..100) and `nextToken` so a paged walk visits every version exactly once. `NextToken` is absent on the final page.
5. **ListApplications** — `GET /applications -> 200`. Must return one `ApplicationSummary` (`ApplicationId`, `Name`, `Author`, `Description`, `CreationTime`, `HomePageUrl`, `Labels`, `SpdxLicenseId`) per application in the store — created ones present, deleted ones absent — with the same `maxItems`/`nextToken` paging contract.
6. **DeleteApplication** — `DELETE /applications/{ApplicationId} -> 204`, empty body. Must remove the application and all of its versions from the store. A subsequent GetApplication must be `NotFoundException` (404), and the application must no longer appear in ListApplications. Deleting an unknown ApplicationId must be 404, not 204.

Every one of these must compute its response from the persisted state reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the request back, no minting an ApplicationId that is never stored: a create followed by a get/list must return exactly what was created, and a delete must be observable by the next caller. Validation that the first slice must enforce: the three `@required` CreateApplication members, the `maxItems` 1..100 range on both list ops (`BadRequestException`, 400), and a malformed/unknown `nextToken` (400).

## State Model

In `ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState`:

- `applications: BTreeMap<String /* ApplicationId ARN */, Application>` — ordered so list paging is stable.
- `name_index: HashMap<String /* Name */, String /* ApplicationId */>` — backs the CreateApplication 409 on duplicate names.
- `Application { application_id: String, name: String, author: String, description: String, home_page_url: Option<String>, labels: Vec<String>, license_url: Option<String>, readme_url: Option<String>, spdx_license_id: Option<String>, is_verified_author: bool, verified_author_url: Option<String>, creation_time: String /* ISO-8601 */, versions: BTreeMap<String /* SemanticVersion */, AppVersion>, latest_version: Option<String> }`
- `AppVersion { semantic_version: String, creation_time: String, template_url: Option<String>, template_body: Option<String>, source_code_url: Option<String>, source_code_archive_url: Option<String>, parameter_definitions: Vec<ParameterDefinition>, required_capabilities: Vec<Capability>, resources_supported: bool }`

Leave room for (but do not implement in this slice) `policy_statements: Vec<ApplicationPolicyStatement>` on `Application` and a `templates: HashMap<TemplateId, CloudFormationTemplate>` map — those back PutApplicationPolicy/GetApplicationPolicy and the CloudFormation template/changeset ops.

Out of scope here, and correctly left returning 501 for later gap issues: UpdateApplication, PutApplicationPolicy, GetApplicationPolicy, UnshareApplication, CreateCloudFormationTemplate, GetCloudFormationTemplate, CreateCloudFormationChangeSet, ListApplicationDependencies.

## Evidence

OPERATIONS.md for this crate (14 operations, restJson1, sigv4 name `serverlessrepo`) gives the routes, required members and error sets quoted above: CreateApplication `POST /applications -> 201` with `Author`/`Description`/`Name` required and `ConflictException (409)` declared; GetApplication `GET /applications/{ApplicationId} -> 200` with `NotFoundException (404)`; CreateApplicationVersion `PUT /applications/{ApplicationId}/versions/{SemanticVersion} -> 201`; ListApplicationVersions and ListApplications both `@paginated(inputToken=NextToken, outputToken=NextToken)` with `MaxItems` constrained `range 1..100`; DeleteApplication `DELETE /applications/{ApplicationId} -> 204`. Shape members for `Version`, `ApplicationSummary` and `VersionSummary` were read from the Smithy model (smithy_query) and are reflected in the state model above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 14 handler files, and `src/state.rs` contains only `// TODO: add service-specific state fields`, proving zero state is held today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI serverlessrepo client at it.
2. CreateApplication Name=my-app Author=someone Description='demo app' Labels=[demo] -> expect 201 with an ApplicationId ARN containing 'applications/my-app' and CreationTime set (today: 501).
3. GetApplication with that ApplicationId -> expect 200 whose Name/Author/Description/Labels match exactly what was created.
4. CreateApplicationVersion ApplicationId=<id> SemanticVersion=1.0.0 TemplateUrl=<url> -> expect 201 echoing SemanticVersion=1.0.0 from the stored version.
5. CreateApplicationVersion again for 1.0.0 -> expect ConflictException 409.
6. ListApplicationVersions ApplicationId=<id> -> expect exactly one VersionSummary with SemanticVersion=1.0.0.
7. ListApplications -> expect my-app present in Applications; repeat with maxItems=1 and follow NextToken to confirm paging covers every application once and the last page omits NextToken.
8. ListApplications with maxItems=0 -> expect BadRequestException 400.
9. DeleteApplication ApplicationId=<id> -> expect 204 with an empty body.
10. GetApplication with the same ApplicationId -> expect NotFoundException 404, and ListApplications no longer contains my-app.
11. DeleteApplication with a never-created ApplicationId -> expect NotFoundException 404.
