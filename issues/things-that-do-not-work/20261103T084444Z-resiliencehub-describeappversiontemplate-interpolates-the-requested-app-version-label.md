Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehub.published-app-version-freezes-resource-mapping
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub.published-app-version-freezes-resource-mapping|run#40|AssertionError|assert version_one_template == release_template_at_publish, (
Issue-Commit: f191aa5d5c1e52cec1d6b06ba3f355db374557d7

# resiliencehub DescribeAppVersionTemplate interpolates the requested app-version label into appTemplateBody's "version", so a published version's template is not frozen

> _Note: this issue was identified against the code base at commit `f191aa5d5c1e52cec1d6b06ba3f355db374557d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppVersionTemplate

## Description
One net-new root cause on resiliencehub's app-version template read path, surfaced by the behavioral suite. Cause 1 of 1.

Cause 1 of 1: the appTemplateBody `version` field is derived from the appVersion the caller READ at, not from the stored snapshot, so the same published version renders a different body depending on the label it is addressed by (failing check: resiliencehub.published-app-version-freezes-resource-mapping).

AWS: PublishAppVersion snapshots the draft, and a published (numbered) app version is immutable thereafter — editing the draft afterwards, or publishing again, must not change the body an already-published version renders. DescribeAppVersionTemplate returns `appTemplateBody` (Smithy: DescribeAppVersionTemplateResponse$appTemplateBody -> AppTemplateBody, @required, length 0..409600, pattern ^[\w\s:,-\.'\/{}\[\]:"\\]+$) whose documented structure is `resources` / `appComponents` / `excludedResources` / `version`; the model's prose for `version` says only "Resilience Hub application version". Recorded behavior: the body of one published snapshot is byte-identical whether that snapshot is addressed through the `release` alias at publish time or through its number (`"1"`) after a later publish moved the alias — i.e. `version` is a property of the stored template document, not of the label in the request. (The recorded wire corpus shows the field rendered as the numeric template schema version `2.0`; that specific value is covered by the already-open issue named below. Independently of its value, it must not vary with the read label.)

Emulator: `DescribeAppVersionTemplate::handle` calls `app_template_body(&input.app_version, &contents)` and the renderer emits `"version": app_version` — the EntityVersion string from the request. Replaying the check: the same published snapshot (identical `resources` and `appComponents`) is returned as `{...,"version":"release"}` when read at publish time and as `{...,"version":"1"}` after a second publish, so the assertion that version 1's template is unchanged once version 2 exists fails with `AssertionError: version 1's template changed once version 2 existed; a numbered version is immutable`.

Why: a client that archives, diffs or signs the template of a published version (change review, drift detection, IaC round-trip through PutDraftAppVersionTemplate) sees the document mutate purely because the newest publish advanced and the version is now addressed by its number instead of the `release` alias. Any byte- or hash-level comparison of a published template reports a spurious change, and a parser that trusts `version` to be the template's schema version reads an app-version label instead.

Fix: the appTemplateBody serializer in the DescribeAppVersionTemplate handler, plus the published-snapshot state it reads — the rendered `version` must come from the stored app-version contents (the template document's own version), not from `input.app_version`, so every read of one published snapshot renders the identical body regardless of the label used to address it.

Related open issues, neither of which covers this: 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md lists this check id but its replay keys are the PublishAppVersion `appVersion`-alias and ConflictException-message assertions (the check now gets past those and fails later, at the template-immutability assertion); 20261030T100305Z-resiliencehub-app-and-app-version-read-responses-diverge.md (wire corpus) covers the appTemplateBody's missing structure (logicalResourceId nulls, appComponents[].id, the appcommon component, excludedResources) and the fact that `version` is a label string rather than the numeric 2.0, but is grounded in wire fixture steps and says nothing about a published snapshot's body changing between two reads.

Traces: resiliencehub.published-app-version-freezes-resource-mapping.

## Evidence
Suite report: resiliencehub 1/11 (9%), 10 failed. All other failures are marked ALREADY FILED (StartAppAssessment scaffold) or OWNED by 20261029T191752Z-resiliencehub-publishappversion-mints-numbered-versions-instead-of-the.md; this is the only surviving cluster.

[mismatch] resiliencehub.published-app-version-freezes-resource-mapping, at check_resiliencehub_published_app_version_freezes_resource_mapping.py:133:
  AssertionError: version 1's template changed once version 2 existed; a numbered version is immutable
  as published (read at appVersion="release", immediately after the first publish):
    {"resources":[{"logicalResourceId":{"identifier":"<bucket-a>"},"type":"AWS::S3::Bucket","name":"<res-a>"},{"logicalResourceId":{"identifier":"<bucket-b>"},"type":"AWS::S3::Bucket","name":"<res-b>"}],"appComponents":[{"name":"<component>","type":"AWS::ResilienceHub::StorageAppComponent","resourceNames":["<res-a>","<res-b>"]}],"version":"release"}
  now (same snapshot, read at appVersion="1" after a second publish):
    {"resources":[ ...identical... ],"appComponents":[ ...identical... ],"version":"1"}

Expected vs actual: expected the two bodies byte-identical (the earlier assertions in the same check already confirmed the release snapshot stays frozen across a draft edit, so `resources`/`appComponents` are correct); actual bodies differ in exactly one field, `version`, which tracks the appVersion string in the request.

Source: services/service-com-amazonaws-resiliencehub-awsresiliencehub/src/ops/describe_app_version_template.rs:192-196 (`app_template_body(&input.app_version, &contents)`) and :270-275 (`"version": app_version` in `app_template_body`). The doc comment at :206-212 states the intent explicitly ("the label this template was read at"), which is the defect.

(Bucket names, resource names, component names and ARNs above are per-run values; only the structure and the invariance of the body across reads are requirements.)

## Steps to Reproduce
1. CreateResiliencyPolicy(policyName=<name>, tier="NonCritical", policy={Software/Hardware/AZ with rtoInSecs/rpoInSecs}) -> policyArn.
2. CreateApp(name=<name>, policyArn=<policyArn>) -> appArn.
3. Create two S3 buckets; CreateAppVersionAppComponent(appArn, name=<component>, type="AWS::ResilienceHub::StorageAppComponent"); CreateAppVersionResource twice (resourceName=<res-a>/<res-b>, logicalResourceId={identifier:<bucket>}, physicalResourceId=<bucket>, resourceType="AWS::S3::Bucket", appComponents=[<component>]).
4. PublishAppVersion(appArn) -> appVersion="release", identifier=1. DescribeAppVersionTemplate(appArn, appVersion="release") and keep the appTemplateBody.
5. DeleteAppVersionResource(appArn, resourceName=<res-b>) on the draft, then PublishAppVersion(appArn) again -> identifier=2 (the alias now resolves to the new snapshot).
6. DescribeAppVersionTemplate(appArn, appVersion="1") and compare with the body kept in step 4. Observe the two bodies differ: `"version":"release"` vs `"version":"1"`, with identical resources/appComponents — the published snapshot's template changed because the label it was read at changed.
7. Full check: run_behavioral_suite() and re-replay resiliencehub.published-app-version-freezes-resource-mapping.
