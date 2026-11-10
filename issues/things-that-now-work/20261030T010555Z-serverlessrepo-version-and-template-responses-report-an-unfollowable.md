Issue-Source: trace-sweep-divergence
Issue-Trace: serverlessapplicationrepository-cloudformation-template-preparation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: serverlessapplicationrepository-cloudformation-template-preparation serverlessapplicationrepository-application-version-lifecycle serverlessrepo-cloudformation-template-generation serverlessrepo-application-version-publish
Issue-Shape-Id: com.amazonaws.serverlessapplicationrepository#ServerlessApplicationRepository
Issue-Key: serverlessrepo|CreateApplication|2xx->2xx|$.version.templateUrl
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->2xx|$.templateUrl
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.version.templateUrl
Issue-Key: serverlessrepo|CreateCloudFormationTemplate|2xx->2xx|$.status
Issue-Key: serverlessrepo|CreateCloudFormationTemplate|2xx->2xx|$.templateUrl
Issue-Key: serverlessrepo|GetCloudFormationTemplate|2xx->2xx|$.templateUrl
Issue-Key: serverlessrepo|CreateApplication|2xx->2xx|$.version.parameterDefinitions[*]
Issue-Key: serverlessrepo|CreateApplicationVersion|2xx->2xx|$.parameterDefinitions[*]
Issue-Key: serverlessrepo|GetApplication|2xx->2xx|$.version.parameterDefinitions[*]
Issue-Commit: d5912a3a34a2e2faa23a606635408f9fea83c969

# serverlessrepo version and template responses report an unfollowable templateUrl, no parameterDefinitions and a PREPARING status

> _Note: this issue was identified against the code base at commit `d5912a3a34a2e2faa23a606635408f9fea83c969`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Every cause below is a serverlessrepo RESPONSE-PAYLOAD divergence on the members that describe a published application version or a prepared CloudFormation template: the artifact locator `templateUrl`, the derived `parameterDefinitions` list, and the prepared template's `status`. The statuses are all correct; the bodies are not. This bundle carries 8 CAUSES and ALL 8 must be closed before submitting — the trace tokens listed with the issue are the replay units for causes 2-8.

Related but NOT covering: the open behavioral issue "serverlessrepo template handling is JSON-only: a YAML SAM template yields no ParameterDefinitions and its missing Transform is never refused" (20261022T182143Z-serverlessapplicationrepository-serverlessrepo-template-handling-is-json-only-a-yaml.md) names the same template-reader root cause as causes 6-8 here, but it is a BEHAVIORAL issue and carries no wire fixture/step, so its replay never exercises these recorded steps; the replay reports them unowned. Also related and not covering: 20261022T011815Z-serverlessrepo-version-publish-and-template-generation-fixtures-404.md (semantic-version message wording).

Cause 1 of 8 — CreateApplication reports a `version.templateUrl` that is not an AWS-shaped artifact locator (fixture serverlessapplicationrepository-cloudformation-template-preparation, step task#1).
AWS: Smithy: CreateApplication POST /applications, success code 201; output member Version (jsonName `version`) whose member TemplateUrl (jsonName `templateUrl`, target __string) is @required with doc "A link to the packaged AWS SAM template of your application." Recorded behavior: for an inline `templateBody`, SAR copies the body into an object in a repository-owned Amazon S3 bucket and reports a presigned GET locator for that object — an https URL whose authority is an S3 endpoint and whose SigV4 query signature is scoped to the `s3` signing service. The corpus asserts this member BY SHAPE, not by literal value, so the recorded bucket name, signature and date are not the requirement; being a followable, S3-presigned object locator is.
Emulator: answers a locator on its own control-plane endpoint, of the form `https://<host>:<port>/serverlessrepo-artifacts/<region>/<account>/<application-name>/<semanticVersion>/template.yaml?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=<key>/<date>/<region>/serverlessrepo/aws4_request&X-Amz-Date=...&X-Amz-Expires=604800&X-Amz-SignedHeaders=host&X-Amz-Signature=...`, which the replay's shape assertion on `$.version.templateUrl` rejects (value_mismatch against the shaped expectation). The host, port, access-key id, date and signature in that example are per-run values and are NOT part of the requirement; the invariant parts that diverge are the `/serverlessrepo-artifacts/...` path space, the credential scope naming `serverlessrepo` instead of `s3`, and the scheme, which `artifact_address` hard-codes to `https` regardless of the scheme the publishing request arrived on. Hypothesis: the shape assertion fails because the announced address is not one the emulator can actually be reached at — a client following an `https://` link into a plaintext listener fails the TLS handshake, i.e. the service announces an address other than the one it is reached on.
Why: `templateUrl` is the one member a CloudFormation/SAM client dereferences to deploy the application. A client that follows the announced link cannot retrieve the packaged template, so the whole publish-then-deploy flow stops at the fetch, and tooling that merely validates the locator's form (an S3 object URL) rejects the response.
Fix: the artifact-locator mint shared by the publish paths — `minted_artifact_url`/`artifact_address` and `resolve_artifact_url` in src/state.rs, called from the CreateApplication handler — must mint a locator of the shape SAR announces and must derive its scheme/authority from the endpoint the request was actually received on instead of hard-coding one.
Traces: serverlessapplicationrepository-cloudformation-template-preparation#task#1.

Cause 2 of 8 — CreateApplicationVersion reports the same non-AWS-shaped `templateUrl` (fixture serverlessapplicationrepository-application-version-lifecycle, step task#2).
AWS: Smithy: CreateApplicationVersion PUT /applications/{ApplicationId}/versions/{SemanticVersion}, success 201; output member TemplateUrl (jsonName `templateUrl`). Recorded behavior as in cause 1: a presigned S3 object locator for the stored packaged template, asserted by shape.
Emulator: the same `/serverlessrepo-artifacts/...` self-addressed, `serverlessrepo`-scoped, hard-coded-`https` locator; `$.templateUrl` fails the shape assertion on the 201 body.
Why: the publish response is where a CI pipeline captures the template locator for the deploy step; an unfollowable locator breaks it at the point of use.
Fix: same mint as cause 1, reached through the CreateApplicationVersion handler (src/ops/create_application_version.rs).
Traces: serverlessapplicationrepository-application-version-lifecycle#task#2.

Cause 3 of 8 — GetApplication reports the same non-AWS-shaped `version.templateUrl` (fixture serverlessapplicationrepository-application-version-lifecycle, step task#7).
AWS: Smithy: GetApplication GET /applications/{ApplicationId}, 200; output member Version whose @required TemplateUrl is read back for the reported version. Recorded behavior: the same presigned S3 object locator the publish announced, asserted by shape.
Emulator: replays the stored self-addressed locator, so `$.version.templateUrl` fails the shape assertion on the read path too.
Why: a client that re-reads an application to find its deployable template gets the same unusable link, so the defect is not confined to the publish response.
Fix: once the mint in cause 1 is corrected, the Version projection in src/state.rs must report that corrected locator (and must not have persisted an environment-specific one).
Traces: serverlessapplicationrepository-application-version-lifecycle#task#7.

Cause 4 of 8 — CreateCloudFormationTemplate answers `status: PREPARING` where AWS answers `ACTIVE`, and reports the cause-1 `templateUrl` (fixture serverlessapplicationrepository-cloudformation-template-preparation, step task#2).
AWS: Smithy: CreateCloudFormationTemplate POST /applications/{ApplicationId}/templates, success 201; output member Status (jsonName `status`) targets the Status enum, values PREPARING | ACTIVE | EXPIRED, doc "Status of the template creation workflow." Recorded behavior: the 201 body of the create itself reports `status` "ACTIVE" — the prepared template is usable as soon as the create returns, with `templateId`, `creationTime`, `expirationTime` and `templateUrl` alongside it. The same step's `templateUrl` is shape-asserted as in cause 1.
Emulator: reports `status` "PREPARING" in the create response (the record is minted with `active_at = created_at + TEMPLATE_PREPARATION_SPAN`, a 500 ms emulator-chosen span, and `status_at` therefore answers PREPARING to any read inside that window), and reports the self-addressed `templateUrl`.
Why: the documented client flow is create-then-poll-until-ACTIVE; a client that instead reads the create response and proceeds (as the recording shows it may) sees a template that is never ACTIVE in that response, and a strict client polls for a transition AWS already completed. Either way the emulator's state machine is observable where AWS's is not.
Fix: the CreateCloudFormationTemplate handler and the `CloudFormationTemplate::prepared`/`status_at` logic in src/state.rs — a freshly prepared template must report ACTIVE from the create response onward (EXPIRED only once `expires_at` passes); plus the locator mint of cause 1.
Traces: serverlessapplicationrepository-cloudformation-template-preparation#task#2.

Cause 5 of 8 — GetCloudFormationTemplate reports the cause-1 `templateUrl` (fixture serverlessapplicationrepository-cloudformation-template-preparation, step task#3).
AWS: Smithy: GetCloudFormationTemplate GET /applications/{ApplicationId}/templates/{TemplateId}, 200; output member TemplateUrl (jsonName `templateUrl`), doc "A link to the template that can be used to deploy the application using AWS CloudFormation." Recorded behavior: the presigned S3 object locator of the prepared template, asserted by shape.
Emulator: `$.templateUrl` is the stored self-addressed locator and fails the shape assertion.
Why: this is the read a CloudFormation deploy uses to obtain the prepared template; an unfollowable locator makes the prepared-template resource useless.
Fix: the locator mint of cause 1, as projected by the prepared-template report (`CloudFormationTemplate::report` in src/state.rs).
Traces: serverlessapplicationrepository-cloudformation-template-preparation#task#3.

Cause 6 of 8 — CreateApplication derives no `version.parameterDefinitions` from a YAML SAM template, and reports the cause-1 `templateUrl` (fixture serverlessrepo-application-version-publish, step setup#1).
AWS: Smithy: CreateApplication 201; Version$ParameterDefinitions (jsonName `parameterDefinitions`, target __listOfParameterDefinition) is @required, doc "An array of parameter types supported by the application."; ParameterDefinition requires Name and ReferencedByResources and carries Type, DefaultValue and the Allowed*/Min*/Max*/NoEcho members. Recorded behavior: for a template declaring a parameter referenced by a resource, SAR derives one entry per declared parameter from THAT version's template — name, type, the default rendered as a string, and the names of the resources referencing it (the recording shows a Number parameter `QueueVisibility` defaulting to "30" referenced by resource `Q`; those names/values come from the submitted template, the derivation rule is the requirement).
Emulator: `$.version.parameterDefinitions[0]` is absent — the list comes back empty, because the template reader only parses bodies that are valid JSON and returns an empty list for anything else, and the fixture publishes YAML.
Why: a client reading an application's parameters before deploying it (the SAM/console deploy flow, parameter-form builders, ParameterOverrides validation) sees a parameterless application and cannot discover required or defaulted parameters or which resources consume them.
Fix: `parse_template` in src/state.rs (and its callers in the CreateApplication / CreateApplicationVersion handlers) must read YAML SAM templates — including the CloudFormation short forms (`!Ref`, `!Sub`) the reference scan depends on — and derive ParameterDefinitions per version; plus the locator mint of cause 1.
Traces: serverlessrepo-application-version-publish#setup#1.

Cause 7 of 8 — CreateApplicationVersion derives no `parameterDefinitions` and reports the cause-1 `templateUrl` (fixture serverlessrepo-application-version-publish, step task#3).
AWS: Smithy: CreateApplicationVersion 201; output ParameterDefinitions (jsonName `parameterDefinitions`) as in cause 6, derived from the template published with THIS version.
Emulator: `$.parameterDefinitions[0]` missing (empty list) for the YAML template, and the self-addressed `templateUrl`.
Why: the publish response is the first place a pipeline reads the version's parameter contract; an empty list silently claims the application takes no parameters.
Fix: as cause 6, on the version-publish path.
Traces: serverlessrepo-application-version-publish#task#3.

Cause 8 of 8 — GetApplication reports no `version.parameterDefinitions` for a YAML-published version, plus the cause-1 `templateUrl` (fixture serverlessrepo-application-version-publish, step task#4).
AWS: Smithy: GetApplication 200; Version$ParameterDefinitions @required, derived per version as in cause 6 and stable across reads.
Emulator: `$.version.parameterDefinitions[0]` missing on the read path as well, since nothing was derived at publish time.
Why: the parameter contract is unavailable both when publishing and when reading back, so no client can recover it.
Fix: as cause 6 — deriving at publish time fixes the read, and the stored derivation must be per version.
Traces: serverlessrepo-application-version-publish#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 8 unowned mismatch clusters, all with the EXPECTED status and a diverging body.

Cause 1 — serverlessapplicationrepository-cloudformation-template-preparation#task#1, CreateApplication, status 201 expected and 201 received; `$.version.templateUrl`: value_mismatch (expected the shaped assertion, got `https://<host>:<port>/serverlessrepo-artifacts/us-west-2/000000000000/<app-name>/1.0.0/template.yaml?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=<key>%2F<date>%2Fus-west-2%2Fserverlessrepo%2Faws4_request&X-Amz-Date=...&X-Amz-Expires=604800&X-Amz-SignedHeaders=host&X-Amz-Signature=...`).
Cause 2 — serverlessapplicationrepository-application-version-lifecycle#task#2, CreateApplicationVersion, 201/201; `$.templateUrl`: same shaped mismatch.
Cause 3 — serverlessapplicationrepository-application-version-lifecycle#task#7, GetApplication, 200/200; `$.version.templateUrl`: same shaped mismatch.
Cause 4 — serverlessapplicationrepository-cloudformation-template-preparation#task#2, CreateCloudFormationTemplate, 201/201; `$.status`: value_mismatch (expected 'ACTIVE', got 'PREPARING'); `$.templateUrl`: same shaped mismatch. Cluster also covers serverlessrepo-cloudformation-template-generation#task#3.
Cause 5 — serverlessapplicationrepository-cloudformation-template-preparation#task#3, GetCloudFormationTemplate, 200/200; `$.templateUrl`: same shaped mismatch. Cluster also covers serverlessrepo-cloudformation-template-generation#task#4.
Cause 6 — serverlessrepo-application-version-publish#setup#1, CreateApplication, 201/201; `$.version.parameterDefinitions[0]`: missing (expected {'name': 'QueueVisibility', 'defaultValue': '30', 'type': 'Number', 'referencedByResources': ['Q']}, got None); `$.version.templateUrl`: same shaped mismatch. Cluster also covers serverlessrepo-cloudformation-template-generation#setup#1.
Cause 7 — serverlessrepo-application-version-publish#task#3, CreateApplicationVersion, 201/201; `$.parameterDefinitions[0]`: missing (same expected entry, for version 1.0.1); `$.templateUrl`: same shaped mismatch.
Cause 8 — serverlessrepo-application-version-publish#task#4, GetApplication, 200/200; `$.version.parameterDefinitions[0]`: missing; `$.version.templateUrl`: same shaped mismatch.

Code: services/service-com-amazonaws-serverlessapplicationrepository-serverlessapplicationrepository/src/state.rs — `artifact_address` (~line 415) formats `"https://{host}{ARTIFACT_PATH_PREFIX}/{region}/{account}/{path}/{file}"` with the scheme hard-coded and ARTIFACT_PATH_PREFIX = "/serverlessrepo-artifacts" (~line 384); `presigned_artifact_url` signs it with the `serverlessrepo` credential scope (~line 571); `parse_template` (~line 1737) does `let Ok(template) = serde_json::from_str::<serde_json::Value>(body) else { return Ok(Vec::new()); }`, so a YAML template yields no ParameterDefinitions; `TEMPLATE_PREPARATION_SPAN` (~line 2104) and `status_at` (~line 2174) make a just-created template report PREPARING.

## Steps to Reproduce
1. Replay fixture serverlessapplicationrepository-cloudformation-template-preparation: step task#1 (CreateApplication with an inline templateBody) — read `$.version.templateUrl` and observe a `/serverlessrepo-artifacts/...` address on the emulator's own endpoint with an `https` scheme and a `serverlessrepo`-scoped SigV4 query signature instead of an AWS-shaped presigned template locator; then step task#2 (CreateCloudFormationTemplate) and observe `$.status` == 'PREPARING' instead of 'ACTIVE'; then step task#3 (GetCloudFormationTemplate) and observe the same `$.templateUrl`.
2. Replay fixture serverlessapplicationrepository-application-version-lifecycle steps task#2 (CreateApplicationVersion) and task#7 (GetApplication) and observe `$.templateUrl` / `$.version.templateUrl` diverging the same way.
3. Replay fixture serverlessrepo-application-version-publish steps setup#1 (CreateApplication), task#3 (CreateApplicationVersion 1.0.1) and task#4 (GetApplication) with the YAML SAM template that declares `Parameters: {QueueVisibility: {Type: Number, Default: 30}}` referenced by resource `Q` via `!Ref`, and observe `parameterDefinitions` empty (the expected first entry missing) in all three responses, plus the same templateUrl divergence.
4. Follow any announced templateUrl: it is not fetchable at the address/scheme announced, which is what the shape assertion on that member rejects.
