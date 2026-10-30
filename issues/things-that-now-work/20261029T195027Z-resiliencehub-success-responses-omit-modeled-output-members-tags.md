Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-lifecycle resiliencehub-assessment-preconditions resiliencehub-resiliency-policy-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateApp|2xx->2xx|$.app.tags
Issue-Key: resiliencehub|DescribeApp|2xx->2xx|$.app.tags
Issue-Key: resiliencehub|UpdateApp|2xx->2xx|$.app.tags
Issue-Key: resiliencehub|CreateResiliencyPolicy|2xx->2xx|$.policy.dataLocationConstraint
Issue-Key: resiliencehub|CreateResiliencyPolicy|2xx->2xx|$.policy.estimatedCostTier
Issue-Key: resiliencehub|CreateResiliencyPolicy|2xx->2xx|$.policy.tags
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.dataLocationConstraint
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.estimatedCostTier
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.tags
Issue-Key: resiliencehub|ListAppVersions|2xx->2xx|$.appVersions[*].identifier
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub success responses omit modeled output members (tags, policy enums, appVersion identifier)

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApp

## Description
Six distinct causes, all the same family of defect: resiliencehub serves a 200 whose body drops modeled members of App, ResiliencyPolicy and AppVersionSummary that AWS always populates. Every status code matches; only the response shape diverges. No open issue covers any of them (the open 'resiliencehub: PublishAppVersion mints numbered versions...' issue is about the version label and two ConflictException texts, not about omitted output members).

=== Cause 1 of 6: CreateApp omits app.tags ===
AWS: CreateApp returns the required `app` member of shape App, which carries `tags` (TagMap). Recorded behavior: an application created with no tags still answers with `tags` PRESENT as an empty map `{}` — the member is serialized, not dropped, so a client can read it unconditionally. (Smithy: CreateApp output app -> App, App.tags target TagMap; the length min 1 constraint is an input constraint, the recording is the authority for the empty-map response.)
Emulator: `$.app.tags` is absent from the body entirely (expected `{}`, got None).
Why: a client that reads the tag map off a create response without a null guard gets a missing-key/None and either throws or treats the application as un-taggable; code that diffs tags between create and a later ListTagsForResource sees a type change rather than an empty set.
Fix: the CreateApp handler's App serializer — always emit `tags`, defaulting to an empty map when the application has none.
Traces: resiliencehub-app-lifecycle#task#2.

=== Cause 2 of 6: DescribeApp omits app.tags ===
AWS: same App shape on the read path; `tags` is present as `{}` for an untagged application.
Emulator: `$.app.tags` missing (expected `{}`, got None) on DescribeApp.
Why: the read path is where clients poll for tag state, so the omission breaks the same unconditional read and makes create/describe round-trips inconsistent with AWS.
Fix: the DescribeApp handler's App serializer (same shared App projection as Cause 1).
Traces: resiliencehub-app-lifecycle#task#3 (also resiliencehub-app-lifecycle#task#6 and resiliencehub-assessment-preconditions#task#6 in the same cluster).

=== Cause 3 of 6: UpdateApp omits app.tags ===
AWS: UpdateApp returns the required `app` (App) and carries `tags` with whatever tags the application holds — in the recording a single tag key/ARN pair, i.e. the member reflects existing state and is never dropped.
Emulator: `$.app.tags` missing where the recorded response carries the application's tag map.
Why: a client reading tags back from the update response sees no tags at all and can conclude its tagging was lost, or overwrite state on the next write.
Fix: the UpdateApp handler's App serializer.
Traces: resiliencehub-app-lifecycle#task#5.

=== Cause 4 of 6: CreateResiliencyPolicy omits policy.dataLocationConstraint, policy.estimatedCostTier and policy.tags ===
AWS: CreateResiliencyPolicy returns the required `policy` of shape ResiliencyPolicy, whose members include `dataLocationConstraint` (enum DataLocationConstraint: AnyLocation|SameContinent|SameCountry), `estimatedCostTier` (enum EstimatedCostTier: L1|L2|L3|L4) and `tags` (TagMap). Recorded behavior: a policy created WITHOUT dataLocationConstraint answers with `dataLocationConstraint` present and resolved to `AnyLocation`; `estimatedCostTier` is present and service-computed (the recorded policy resolves to `L3`); `tags` is present as `{}` for an untagged policy. Hypothesis: estimatedCostTier is derived from the policy's RTO/RPO values and tier — the model documents only that the service specifies it, so the requirement is that the member is always populated with a valid EstimatedCostTier value for the submitted policy, not the single recorded letter.
Emulator: all three members absent from the 200 body.
Why: dataLocationConstraint and estimatedCostTier are how a client reports a policy's geography constraint and cost class; absent members read as 'unknown' and any client or console view that renders them (or compares policies by cost tier) gets nothing. The missing `tags` breaks the same unconditional read as Causes 1-3.
Fix: the CreateResiliencyPolicy handler's ResiliencyPolicy serializer plus the policy state record — it must default dataLocationConstraint, compute/store estimatedCostTier, and default tags to an empty map.
Traces: resiliencehub-resiliency-policy-lifecycle#task#1.

=== Cause 5 of 6: DescribeResiliencyPolicy omits the same three members ===
AWS: the read path returns the identical ResiliencyPolicy projection, so dataLocationConstraint, estimatedCostTier and tags are present.
Emulator: `$.policy.dataLocationConstraint`, `$.policy.estimatedCostTier` and `$.policy.tags` all missing on DescribeResiliencyPolicy.
Why: a client cannot discover a policy's constraint or cost tier at all, even on a dedicated read, so policy inventory/reporting against this emulator is empty where AWS is populated.
Fix: the DescribeResiliencyPolicy handler's ResiliencyPolicy serializer (shared projection with Cause 4).
Traces: resiliencehub-resiliency-policy-lifecycle#task#2.

=== Cause 6 of 6: ListAppVersions omits AppVersionSummary.identifier ===
AWS: ListAppVersions returns `appVersions` of AppVersionSummary, which carries `identifier` (LongOptional, 'Identifier of the application version') alongside `appVersion`. Recorded behavior: the `draft` entry carries `identifier` = 0 and each published entry carries its monotonically increasing publish sequence number, so the identifier is always present for every listed version.
Emulator: `$.appVersions[0].identifier` is absent (expected 0, got None) for the draft entry.
Why: the identifier is the only ordering key a client has for app versions — the version label itself is an opaque string. Without it, a client cannot tell which listed version is newest or correlate a PublishAppVersion response's `identifier` with a list entry.
Fix: the ListAppVersions handler's AppVersionSummary serializer and the app-version state record (the draft must carry identifier 0, published snapshots their sequence number).
Traces: resiliencehub-assessment-preconditions#task#8. The same omission also appears on resiliencehub-app-version-publish#task#10, whose cluster is filed with the PublishAppVersion version-label issue because that step additionally diverges on the version label.

## Evidence
All steps answered HTTP 200 with the expected status; the divergence is purely missing body members.

Cause 1 — CreateApp, resiliencehub-app-lifecycle#task#2:
  $.app.tags: missing (expected {}, got None)
Cause 2 — DescribeApp, resiliencehub-app-lifecycle#task#3 (cluster also covers resiliencehub-app-lifecycle#task#6, resiliencehub-assessment-preconditions#task#6):
  $.app.tags: missing (expected {}, got None)
Cause 3 — UpdateApp, resiliencehub-app-lifecycle#task#5:
  $.app.tags: missing (expected a one-entry tag map, got None)
Cause 4 — CreateResiliencyPolicy, resiliencehub-resiliency-policy-lifecycle#task#1:
  $.policy.dataLocationConstraint: missing (expected 'AnyLocation', got None)
  $.policy.estimatedCostTier: missing (expected 'L3', got None)
  $.policy.tags: missing (expected {}, got None)
Cause 5 — DescribeResiliencyPolicy, resiliencehub-resiliency-policy-lifecycle#task#2: identical three omissions.
Cause 6 — ListAppVersions, resiliencehub-assessment-preconditions#task#8:
  $.appVersions[0].identifier: missing (expected 0, got None)

Smithy citations: App.tags -> TagMap; ResiliencyPolicy.dataLocationConstraint -> DataLocationConstraint enum [AnyLocation, SameContinent, SameCountry]; ResiliencyPolicy.estimatedCostTier -> EstimatedCostTier enum [L1, L2, L3, L4]; ResiliencyPolicy.tags -> TagMap; AppVersionSummary.identifier -> LongOptional. ARNs, tag values and the recorded cost-tier letter are per-run values filling the shape; the requirement is member presence and a valid resolved value.

## Steps to Reproduce
1. CreateApp(name=<name>) with no tags. Inspect the response body: `app.tags` is absent instead of `{}` (Cause 1).
2. DescribeApp(appArn=<arn from step 1>): `app.tags` absent (Cause 2).
3. UpdateApp(appArn=<arn>, description=<text>) on an application that holds tags: `app.tags` absent (Cause 3).
4. CreateResiliencyPolicy(policyName=<name>, tier="NonCritical", policy={Software/Hardware/AZ each with rtoInSecs/rpoInSecs}) with no dataLocationConstraint and no tags: response `policy` has no dataLocationConstraint, no estimatedCostTier, no tags (Cause 4).
5. DescribeResiliencyPolicy(policyArn=<arn from step 4>): same three members absent (Cause 5).
6. ListAppVersions(appArn=<arn from step 1>): the `draft` entry has no `identifier` member (Cause 6).
Full replay: replay_wire_corpus() and re-check resiliencehub-app-lifecycle#task#2, #task#3, #task#5, resiliencehub-resiliency-policy-lifecycle#task#1, #task#2, resiliencehub-assessment-preconditions#task#8.
