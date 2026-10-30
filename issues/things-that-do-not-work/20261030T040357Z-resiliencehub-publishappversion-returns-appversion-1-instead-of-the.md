Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.published-version-freezes-resource-mapping
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub.published-version-freezes-resource-mapping|run#11|AssertionError|assert v1["appVersion"] == "release", (
Issue-Commit: 2d2dbabb1e5bace0c65b702057f0f7e9ab3f6c6a

# resiliencehub: PublishAppVersion returns appVersion '1' instead of the 'release' alias, so a published template cannot be read back at appVersion='release'

> _Note: this issue was identified against the code base at commit `2d2dbabb1e5bace0c65b702057f0f7e9ab3f6c6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishAppVersion

## Description
One root cause, grounded in the one behavioral check that survived the report's ALREADY-FILED/OWNED markings for this service (the other ten resiliencehub failures are marked OWNED by 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md or ALREADY FILED against the unimplemented StartAppAssessment stub, and are not re-filed here).

Related open issues that describe the same misbehaviour but do NOT carry this check's replay key: 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md (its Cause 1) and 20261029T195027Z-resiliencehub-publishappversion-labels-the-published-version-by-sequence.md. Neither lists resiliencehub.published-version-freezes-resource-mapping in its trace ids, so this check is still ungated; fixing either issue should be confirmed by re-replaying the id in trace_ids below.

Cause 1 of 1: PublishAppVersion labels the published snapshot with its sequence number instead of the constant alias 'release'.

AWS: PublishAppVersion snapshots the DRAFT app version and addresses the newest published snapshot by the fixed alias string `release`; the monotonically increasing publish sequence number is returned separately. Smithy (com-amazonaws-resiliencehub-awsresiliencehub): PublishAppVersion output carries BOTH `appVersion` (EntityVersion, pattern ^\S{1,50}$) and `identifier` (LongOptional) — two distinct members precisely because the version LABEL and the sequence NUMBER are not the same value; `versionName` is echoed back separately as well. DescribeAppVersionTemplate takes a required `appVersion` (EntityVersion) and is the documented way to read a published template, with `release` naming the current release. Recorded behavior: the first publish of a non-empty, resolved draft answers appVersion="release" with identifier=1, and DescribeAppVersionTemplate(appArn, appVersion="release") then returns exactly the resources the draft held at publish time, while the superseded snapshot stays retrievable under its own numeric appVersion.

Emulator: the publish handler derives the version label from the sequence number and stores the snapshot under that numeric key, so the first publish answers appVersion='1'. Replay output: `AssertionError: publish put the app template at '1', expected 'release'` raised at the publish call in check_resiliencehub_published_version_freezes_resource_mapping.py. The string `release` therefore names no version in the emulator's state, and the check's subsequent DescribeAppVersionTemplate(appVersion="release") read of the frozen template cannot be reached at all.

Why: a client that publishes and then reads or assesses its current release is broken. `release` is the appVersion clients pass to DescribeAppVersionTemplate, ListAppVersionResources, ResolveAppVersionResources and StartAppAssessment; against this emulator that value resolves to nothing, so every post-publish read fails and the 'draft vs current release' distinction a client relies on (ListAppVersions reporting {draft, release}) is replaced by {draft, <n>}. The freeze semantics this check grades — the release template staying unchanged while the draft accumulates further resources — are consequently unobservable.

Fix: the PublishAppVersion handler and the app-version keying in this service's state: the published snapshot must be inserted and addressed under the constant alias for the current release, with the publish sequence number carried only in the `identifier` output member (and a re-publish replacing what the alias resolves to while the superseded snapshot remains readable under its number).

Traces: resiliencehub.published-version-freezes-resource-mapping. Code citation: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/publish_app_version.rs:205-222 (`let identifier = store.next_published_app_version(...); let app_version = identifier.to_string();` then `store.insert_app_version(&input.app_arn, &app_version, published)` and `app_version: Some(app_version)` in the output).

## Evidence
Suite report: resiliencehub 0/11 (0%), 11 failed. Surviving (not OWNED, not ALREADY FILED) failure:

[mismatch] resiliencehub.published-version-freezes-resource-mapping
  File "check_resiliencehub_published_version_freezes_resource_mapping.py", line 123, in run
    assert v1["appVersion"] == "release", (
  AssertionError: publish put the app template at '1', expected 'release'

Expected vs actual (HTTP 200 in both cases):
  expected: PublishAppVersion(appArn=<arn>, versionName=<name>) -> {appVersion: "release", identifier: 1, versionName: <name>}; DescribeAppVersionTemplate(appArn=<arn>, appVersion="release") -> the template carrying exactly the resources mapped before the publish.
  actual:   PublishAppVersion -> {appVersion: "1", identifier: 1}; no version is addressable as "release".

The literal '1' is the per-run sequence number filling the label slot; the requirement is that the label be the fixed release alias and that the number appear only in `identifier`. ARNs, bucket names and generated app names in the replay are per-run values and are not part of the requirement.

## Steps to Reproduce
1. CreateApp(name=<app-name>) -> appArn.
2. CreateBucket(<bucket>) in the test region; CreateAppVersionAppComponent(appArn, name=<component>, id=<component>, type="AWS::ResilienceHub::StorageAppComponent"); CreateAppVersionResource(appArn, appComponents=[<component>], resourceType="AWS::S3::Bucket", logicalResourceId={identifier: <bucket>}, physicalResourceId=<bucket>, resourceName=<bucket>, awsRegion=<region>, awsAccountId=<account>).
3. ResolveAppVersionResources(appArn, appVersion="draft") and poll DescribeAppVersionResourcesResolutionStatus to Success.
4. PublishAppVersion(appArn, versionName="v1"). Observe appVersion=="1" in the response instead of "release" (identifier==1 is correct).
5. DescribeAppVersionTemplate(appArn, appVersion="release") — against real AWS this returns the frozen template just published; against the emulator "release" addresses no version, so the frozen-template read (and the later comparison of the release template against a mutated draft) cannot be performed.

Full replay: run_behavioral_suite() and re-run resiliencehub.published-version-freezes-resource-mapping.
