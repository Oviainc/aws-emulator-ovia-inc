Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-publish
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-publish
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|PublishAppVersion|2xx->2xx|$.appVersion
Issue-Key: resiliencehub|ListAppVersions|2xx->2xx|$.appVersions[*].appVersion
Issue-Related: 20261029T195027Z-resiliencehub-success-responses-omit-modeled-output-members-tags.md
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub PublishAppVersion labels the published version by sequence number instead of the release alias

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishAppVersion

## Description
Two causes, one root fault, now confirmed on the WIRE corpus (an open behavioral issue reports the same fault from the behavioral suite — see below — but carries no wire replay key, so this wire divergence is unfiled and unreproducible from that issue).

=== Cause 1 of 2: PublishAppVersion returns appVersion = the sequence number ===
AWS: PublishAppVersion output carries `appVersion` (EntityVersion, pattern ^\S{1,50}$) and `identifier` (LongOptional) as two SEPARATE members, precisely because the version label and the publish sequence number are different values. Recorded behavior: publishing a non-empty draft answers `appVersion` = the fixed alias string `release`, with the sequence number carried in `identifier`; the newest published snapshot is always addressed by that alias and a re-publish replaces what `release` resolves to.
Emulator: answers `appVersion` = `'1'`, i.e. the sequence number stringified, and stores/addresses the snapshot under that number.
Why: every downstream call that takes an appVersion — DescribeAppVersionTemplate, ListAppVersionResources, DescribeAppVersionResource, ResolveAppVersionResources, StartAppAssessment — is used with `appVersion="release"`. Against this emulator that string names no version, so all post-publish reads and assessments of a published application fail, and a client cannot address its own release at all.
Fix: the PublishAppVersion handler and this service's app-version keying in state — store and report the published snapshot under the constant alias `release`, carrying the sequence number only in `identifier`.
Traces: resiliencehub-app-version-publish#task#9.

=== Cause 2 of 2: ListAppVersions reports the numeric label (and still omits identifier) ===
AWS: ListAppVersions returns AppVersionSummary entries whose `appVersion` is the version label and whose `identifier` (LongOptional) is the ordering key. Recorded behavior after one publish: exactly two entries — `draft` with identifier 0, and `release` with identifier 1.
Emulator: the second entry's `appVersion` is `'1'` instead of `'release'`, and the draft entry has no `identifier` member at all (expected 0, got None).
Why: a client that distinguishes the draft from the current release by label sees a version set of {draft, 1} it cannot interpret, and with `identifier` absent it has no fallback ordering key either — so neither label-based nor identifier-based selection of the newest version works.
Fix: the same app-version keying as Cause 1, plus the ListAppVersions AppVersionSummary serializer, which must populate `identifier` for every entry (0 for the draft). The identifier omission is the same serializer defect filed as Cause 6 of the 'resiliencehub success responses omit modeled output members' issue; this cluster is filed here because its primary divergence is the version label.
Traces: resiliencehub-app-version-publish#task#10.

Relation to existing issues: the OPEN issue 'resiliencehub: PublishAppVersion mints numbered versions instead of the "release" alias, and both ConflictException refusal messages diverge from AWS wording' describes the same root cause from the behavioral corpus (Issue-Corpus: simparity-behavioral-aws-v1, check ids resiliencehub.published-app-version-freezes-resource-mapping etc.). It carries NO Issue-Trace/Issue-Step for this wire corpus, and the replay reported these two clusters as unowned, so without this issue a fixer has no wire fixture step to re-replay for them.

## Evidence
Both steps returned the expected HTTP 200; the version label diverges.

Cause 1 — PublishAppVersion, resiliencehub-app-version-publish#task#9:
  $.appVersion: value_mismatch (expected 'release', got '1')
Cause 2 — ListAppVersions, resiliencehub-app-version-publish#task#10:
  $.appVersions[0].identifier: missing (expected 0, got None)
  $.appVersions[1].appVersion: value_mismatch (expected 'release', got '1')

Smithy citations: PublishAppVersion output members appVersion (EntityVersion ^\S{1,50}$) and identifier (LongOptional) are distinct; AppVersionSummary.appVersion (required EntityVersion) and AppVersionSummary.identifier (LongOptional, 'Identifier of the application version'). `release` is the fixed alias in the recorded responses and has no per-run variation; the sequence number 1 is the per-run publish count and belongs in `identifier`.

## Steps to Reproduce
1. CreateResiliencyPolicy(policyName=<p>, tier="NonCritical", policy={Software/Hardware/AZ with rtoInSecs/rpoInSecs}) -> policyArn.
2. CreateApp(name=<app>, policyArn=<policyArn>) -> appArn.
3. CreateAppVersionAppComponent(appArn, name=<c>, type="AWS::ResilienceHub::StorageAppComponent") then CreateAppVersionResource(appArn, resourceName=<r>, logicalResourceId={identifier:<bucket>}, physicalResourceId=<bucket>, resourceType="AWS::S3::Bucket", appComponents=[<c>]).
4. PublishAppVersion(appArn). Observe appVersion == '1' instead of 'release' (Cause 1).
5. ListAppVersions(appArn). Observe entries {draft, '1'} instead of {draft, release}, and the draft entry with no identifier (Cause 2).
6. Any subsequent call with appVersion="release" (DescribeAppVersionTemplate / ListAppVersionResources / StartAppAssessment) cannot address the published version.
Full replay: replay_wire_corpus() and re-check resiliencehub-app-version-publish#task#9 and #task#10.
