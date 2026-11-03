Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-draft-composition
Issue-Step: task#12
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-draft-composition resiliencehub-app-version-publish
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|ResolveAppVersionResources|2xx->2xx|$.resolutionId
Issue-Commit: 38a893cb906a213bf779a9e5f05354914555db8b

# resiliencehub ResolveAppVersionResources returns a resolutionId whose shape diverges from AWS

> _Note: this issue was identified against the code base at commit `38a893cb906a213bf779a9e5f05354914555db8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResolveAppVersionResources

## Description
One net-new cause on resiliencehub's resource-resolution path. The step reaches the handler and answers the modeled HTTP 200 with every other member matching (appArn, appVersion, status all pass); what diverges is the SHAPE of the server-generated `resolutionId` the response carries. The replay harness does not assert this member's literal value (it is server-generated and differs per run) — it asserts the shape of the recorded value, and the emulator's value fails that shape assertion, so this is not a per-run-value difference. No open issue covers it: 'resiliencehub app and app-version read responses diverge from AWS: template body, UpdateApp tags, version-not-found wording' (20261030T100305Z) covers DescribeAppVersionTemplate/UpdateApp bodies, 'resiliencehub accepts invalid app-version and policy input with 200 instead of the modeled refusal' (20261029T195027Z) covers missing refusals on CreateAppVersion*/CreateResiliencyPolicy, and 'resiliencehub refusal messages diverge from AWS wording across app, resource and policy errors' (20261029T195027Z) covers refusal wording — none of them touches ResolveAppVersionResources' success body.

=== Cause 1 of 1: the minted resolutionId does not have the shape AWS's resolutionId has ===
AWS: ResolveAppVersionResources answers HTTP 200 with appArn (Arn, @required), appVersion (EntityVersion, pattern ^\S{1,50}$, @required), resolutionId (String255, @length 1..255, @required) and status (ResourceResolutionStatusType, one of Pending|InProgress|Failed|Success, @required). Smithy: ResolveAppVersionResourcesResponse$resolutionId -> String255, @required, length 1..255; its documentation says only 'The identifier for a specific resolution.' — the model constrains nothing about the identifier's FORMAT, so the requirement here is recorded behavior only: the fixture's recorded response carries a resolutionId of a particular shape, which the corpus asserts as a shape rather than as a literal, and the identifier must be opaque, unique and accepted back by DescribeAppVersionResourcesResolutionStatus(resolutionId=...).
Emulator: mints a bare canonical 36-character hyphenated version-4 UUID for resolutionId (crate state helper `mint_resource_id()`, which is `Uuid::new_v4().hyphenated().to_string()`, shared with application and resiliency-policy id minting). The replay reports `$.resolutionId: value_mismatch (expected '<shaped-not-asserted>', got <a 36-char hyphenated UUID>)` on an otherwise correct 200 body — i.e. the recorded AWS identifier does not have the bare-UUID shape, so re-using the generic app/policy id minter for this member is what diverges. Hypothesis: AWS's resolutionId is a differently structured opaque token (not the same 8-4-4-4-12 UUID form Resilience Hub uses for app and resiliency-policy ids); the authoritative shape is the recorded resolutionId in the fixture step named under Traces, which a fixer should read off the recording rather than guess.
Why: resolutionId is the handle a client carries from ResolveAppVersionResources to DescribeAppVersionResourcesResolutionStatus and to resolution-scoped reads, and clients and IaC providers routinely validate or key on an identifier's shape (regex guards on inputs, fixtures and snapshot tests, id-type sniffing that distinguishes an app id from a resolution handle). A client that accepts AWS-shaped resolution handles rejects or mis-classifies the emulator's, and code that infers 'this is an app/policy UUID' from the shape treats a resolution handle as a resource id — while anything asserting on the AWS shape fails only against the emulator.
Fix: the id-minting seam the ResolveAppVersionResources handler reaches, i.e. the resolution record's constructor in this service's state rather than the handler body — the handler just returns `resolution.resolution_id`, and the constructor calls the generic resource-id minter. Give a resource resolution its own minter that produces AWS's resolutionId shape, keeping it opaque, unique within a run and across restarts, carrying no ordering or minting instant, and still resolvable by `resolution()` / `resolution_mut()` so DescribeAppVersionResourcesResolutionStatus keeps round-tripping it. Code sites for the fixer: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/resolve_app_version_resources.rs lines 193-201 (handler), services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/state.rs lines 245-252 (`ResourceResolution::started_at`) and lines 804-820 (`mint_resource_id`).
Traces: resiliencehub-app-version-draft-composition#task#12 and resiliencehub-app-version-publish#task#6 (same cluster, affected_count 2).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health answered 200, data listener accepting; bootstrap_signal false). Cluster key: service=resiliencehub|ResolveAppVersionResources|status=200->200|keys=$.resolutionId, verdict mismatch, affected_count 2.

ResolveAppVersionResources, HTTP 200 expected and returned, resiliencehub-app-version-draft-composition#task#12 (also resiliencehub-app-version-publish#task#6):
  $.resolutionId: value_mismatch (expected '<shaped-not-asserted>', got a bare 36-char hyphenated version-4 UUID)
  Every other asserted member of the 200 body matched: $.appArn, $.appVersion and $.status.
The expectation is a SHAPE assertion, not a literal: the harness prints '<shaped-not-asserted>' precisely because the recorded identifier is server-generated and its per-run value is not a requirement. The requirement that fails is the shape class of the recorded resolutionId; the emulator's hyphenated-UUID form is outside it. The UUID in any one replay is a per-run value and is not part of the requirement.

Smithy citations (services/service-com-amazonaws-resiliencehub-awsresiliencehub/smithy-model/com-amazonaws-resiliencehub-awsresiliencehub.json): ResolveAppVersionResources http POST /resolve-app-version-resources, success 200; ResolveAppVersionResourcesResponse$resolutionId -> String255 (@required, @length min 1 max 255), doc 'The identifier for a specific resolution.'; ResolveAppVersionResourcesResponse$status -> ResourceResolutionStatusType (Pending|InProgress|Failed|Success); DescribeAppVersionResourcesResolutionStatusRequest$resolutionId -> String255 (optional) — the same identifier is read back on the status operation, so whatever shape is minted must remain resolvable there. The model states no format for the identifier, hence 'recorded behavior only' above.

## Steps to Reproduce
1. CreateApp(name=<name>) -> HTTP 200, note appArn.
2. CreateAppVersionAppComponent(appArn=<arn>, type=AWS::ResilienceHub::StorageAppComponent, ...) and CreateAppVersionResource(appArn=<arn>, resourceType=AWS::S3::Bucket, ...) against the draft version.
3. ResolveAppVersionResources(appArn=<arn>, appVersion="draft") -> HTTP 200 whose $.resolutionId is a bare 36-character hyphenated version-4 UUID instead of an identifier of the shape AWS's recorded resolutionId has; $.appArn, $.appVersion and $.status all match.
4. The same divergence appears on the published path: ResolveAppVersionResources(appArn=<arn>, appVersion="release"/<published version>) (resiliencehub-app-version-publish#task#6).
5. Replay resiliencehub-app-version-draft-composition#task#12 (and resiliencehub-app-version-publish#task#6) and read the recorded resolutionId to recover the required shape; then re-run replay_wire_corpus() and confirm the cluster service=resiliencehub|ResolveAppVersionResources|status=200->200|keys=$.resolutionId is gone and that DescribeAppVersionResourcesResolutionStatus still resolves the newly shaped id.
