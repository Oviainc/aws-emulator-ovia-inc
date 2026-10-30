Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-draft-composition
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-draft-composition resiliencehub-app-version-publish
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateAppVersionAppComponent|2xx->2xx|$.appComponent.additionalInfo
Issue-Key: resiliencehub|CreateAppVersionAppComponent|2xx->2xx|$.appComponent.id
Issue-Key: resiliencehub|CreateAppVersionResource|2xx->2xx|$.physicalResource.appComponents[*].id
Issue-Key: resiliencehub|CreateAppVersionResource|2xx->2xx|$.physicalResource.physicalResourceId.awsAccountId
Issue-Key: resiliencehub|CreateAppVersionResource|2xx->2xx|$.physicalResource.physicalResourceId.awsRegion
Issue-Key: resiliencehub|DescribeAppVersionResource|2xx->2xx|$.physicalResource.appComponents[*].id
Issue-Key: resiliencehub|DescribeAppVersionResource|2xx->2xx|$.physicalResource.physicalResourceId.awsAccountId
Issue-Key: resiliencehub|DescribeAppVersionResource|2xx->2xx|$.physicalResource.physicalResourceId.awsRegion
Issue-Key: resiliencehub|CreateAppVersionResource|2xx->2xx|$.physicalResource.resourceName
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub mints synthetic AppComponent ids and drops physicalResource identity fields

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppVersionAppComponent

## Description
Four causes sharing one root fault: the emulator does not use the client-supplied Application Component name as the component's id, and it does not populate the PhysicalResource identity members it stores. All four steps answer 200, so the divergence is in how the entities are identified and projected.

=== Cause 1 of 4: CreateAppVersionAppComponent replaces the component id with a generated token and omits additionalInfo ===
AWS: CreateAppVersionAppComponent takes a required `name` (String255) and an OPTIONAL `id` (String255), and returns `appComponent` of shape AppComponent whose `id` is an EntityName255 (pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{0,254}$). Recorded behavior: when the request supplies `name` and no `id`, the returned `appComponent.id` equals the supplied NAME — the component is addressable by the identifier the client chose. The response also carries `additionalInfo` present as an empty map `{}` when the request supplied none.
Emulator: returns `appComponent.id` = a service-minted token of the form `appcomponent-<uuid>` that bears no relation to the request, and omits `additionalInfo` entirely.
Why: every downstream call addresses components by this id — CreateAppVersionResource.appComponents is a list of component NAMES/ids, and DeleteAppVersionAppComponent/DescribeAppVersionAppComponent take `id`. A client that created a component called <name> and then references <name> cannot resolve it, and it has no way to predict the emulator's generated token without parsing it back out of the create response.
Fix: the CreateAppVersionAppComponent handler and the app-component state key — default `id` to the request's `name` when `id` is absent (keep an explicit request `id` when given), and serialize `additionalInfo` as an empty map rather than omitting it.
Traces: resiliencehub-app-version-draft-composition#task#4 (cluster also covers resiliencehub-app-version-publish#task#3).

=== Cause 2 of 4: CreateAppVersionResource reports the generated component token in physicalResource.appComponents and omits physicalResourceId.awsAccountId / awsRegion ===
AWS: CreateAppVersionResource returns `physicalResource` (PhysicalResource) whose `appComponents` is a list of AppComponent and whose required `physicalResourceId` is a PhysicalResourceId with `identifier`, `type`, `awsRegion` (AwsRegion) and `awsAccountId` (CustomerId, ^[0-9]{12}$). Recorded behavior: `appComponents[0].id` is the component's client-chosen name, and `physicalResourceId` carries BOTH `awsAccountId` (the calling account) and `awsRegion` (the request's/caller's region) — resolved from the request when supplied and defaulted to the caller's account and region otherwise.
Emulator: `appComponents[0].id` is the `appcomponent-<uuid>` token from Cause 1, and `physicalResourceId.awsAccountId` / `physicalResourceId.awsRegion` are both absent from the body.
Why: the account/region pair is what makes a physical resource identifier unique across a multi-account application; DescribeAppVersionResource accepts `awsAccountId`/`awsRegion` as lookup keys, so a client that reads them back as null cannot re-address the resource it just created. The wrong component id breaks the component-to-resource mapping a client built.
Fix: the CreateAppVersionResource handler's PhysicalResource serializer and the resource state record — resolve and store awsAccountId/awsRegion (request value, else caller's account/region) and project the component's real id.
Traces: resiliencehub-app-version-draft-composition#task#7.

=== Cause 3 of 4: DescribeAppVersionResource shows the same two defects on the read path ===
AWS: DescribeAppVersionResource returns the identical PhysicalResource projection, so the component id is the client-chosen one and physicalResourceId carries awsAccountId and awsRegion.
Emulator: `$.physicalResource.appComponents[0].id` is the generated `appcomponent-<uuid>` token and `physicalResourceId.awsAccountId` / `awsRegion` are missing.
Why: the read path is where a client reconciles its own model of the application against the service; with no account/region and an unrecognisable component id the reconciliation always reports drift.
Fix: the DescribeAppVersionResource handler's PhysicalResource serializer (shared projection with Cause 2).
Traces: resiliencehub-app-version-draft-composition#task#8.

=== Cause 4 of 4: CreateAppVersionResource omits physicalResource.resourceName ===
AWS: PhysicalResource.resourceName is an EntityName (pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$) and CreateAppVersionResource takes `resourceName` as input. Recorded behavior: the response echoes the submitted `resourceName`, and `appComponents[0].id` is again the client-chosen component name.
Emulator: `physicalResource.resourceName` is absent from the 200 body, and `appComponents[0].id` is the generated token.
Why: resourceName is the handle used by DescribeAppVersionResource/DeleteAppVersionResource; a client that submits a name and reads back none must assume the service rejected or renamed it, and cannot address the resource by name afterwards.
Fix: the CreateAppVersionResource handler — echo the request's resourceName into the stored resource and into the PhysicalResource projection.
Traces: resiliencehub-app-version-publish#task#4.

## Evidence
All four steps returned the expected HTTP 200; the bodies diverge.

Cause 1 — CreateAppVersionAppComponent, resiliencehub-app-version-draft-composition#task#4:
  $.appComponent.additionalInfo: missing (expected {}, got None)
  $.appComponent.id: value_mismatch (expected the request's component name, got 'appcomponent-18e317ab-86ea-bfc1-0000-000000000002')
Cause 2 — CreateAppVersionResource, resiliencehub-app-version-draft-composition#task#7:
  $.physicalResource.appComponents[0].id: value_mismatch (expected the component name, got 'appcomponent-18e317ab-86ea-bfc1-0000-000000000002')
  $.physicalResource.physicalResourceId.awsAccountId: missing (expected the caller's 12-digit account, got None)
  $.physicalResource.physicalResourceId.awsRegion: missing (expected the caller's region, got None)
Cause 3 — DescribeAppVersionResource, resiliencehub-app-version-draft-composition#task#8: identical three diffs.
Cause 4 — CreateAppVersionResource, resiliencehub-app-version-publish#task#4:
  $.physicalResource.appComponents[0].id: value_mismatch (expected the component name, got 'appcomponent-18e317ab-c095-587a-0000-000000000001')
  $.physicalResource.resourceName: missing (expected the submitted resourceName, got None)

Smithy citations: CreateAppVersionAppComponent input name (required, String255) / id (optional, String255); AppComponent.id -> EntityName255 ^[A-Za-z0-9][A-Za-z0-9_\-]{0,254}$; AppComponent.additionalInfo -> AdditionalInfoMap; PhysicalResource.resourceName -> EntityName; PhysicalResourceId members identifier/type/awsRegion(AwsRegion)/awsAccountId(CustomerId ^[0-9]{12}$). The specific component names, uuids, account 000000000000 and region us-west-2 in the recording are per-run values; the requirement is that id equals the client-chosen name and that the account/region/name members are populated.

## Steps to Reproduce
1. CreateApp(name=<app>) -> appArn.
2. CreateAppVersionAppComponent(appArn, name=<component>, type="AWS::ResilienceHub::StorageAppComponent") with no `id` and no `additionalInfo`. Observe appComponent.id == 'appcomponent-<uuid>' instead of <component>, and no additionalInfo member (Cause 1).
3. CreateAppVersionResource(appArn, resourceName=<res>, logicalResourceId={identifier:<bucket>}, physicalResourceId=<bucket>, resourceType="AWS::S3::Bucket", appComponents=[<component>]). Observe physicalResource.appComponents[0].id is the generated token, physicalResourceId has no awsAccountId/awsRegion (Cause 2) and, as in resiliencehub-app-version-publish#task#4, no resourceName (Cause 4).
4. DescribeAppVersionResource(appArn, appVersion="draft", logicalResourceId={identifier:<bucket>}). Observe the same component-id and account/region diffs (Cause 3).
Full replay: replay_wire_corpus() and re-check resiliencehub-app-version-draft-composition#task#4, #task#7, #task#8 and resiliencehub-app-version-publish#task#4.
