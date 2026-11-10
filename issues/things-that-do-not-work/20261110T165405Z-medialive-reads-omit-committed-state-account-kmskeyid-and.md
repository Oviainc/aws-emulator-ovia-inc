Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-account-configuration-roundtrip
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-account-configuration-roundtrip medialive-cloudwatch-alarm-template-group-lifecycle
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DescribeAccountConfiguration|2xx->2xx|$.accountConfiguration.kmsKeyId
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-CloudFrontGeneralWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectArqProtocolWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectCdiJpegxsWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectInputFailoverWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaConnectRtpWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveMultiplexWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveToMediaPackageWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaLiveUdpAndRtmpWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaPackageGeneralWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaPackageGeneralWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].arn
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].id
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="AWS-MediaTailorAdInsertionWorkflows"].templateCount
Issue-Key: medialive|ListCloudWatchAlarmTemplateGroups|2xx->2xx|$.cloudWatchAlarmTemplateGroups[name="sim2real-probe-medialive-cwgroup-7f786eeb"]
Issue-Commit: c5aebe6e0c0512dca8137d34d1d8507ed568175c

# medialive reads omit committed state: account kmsKeyId and a created alarm template group are absent from the reads that follow the write

> _Note: this issue was identified against the code base at commit `c5aebe6e0c0512dca8137d34d1d8507ed568175c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccountConfiguration

## Description
Two net-new wire divergences in the medialive emulator, BOTH of the same shape: a read answers 200 but omits state that the write preceding it committed, so a client cannot read back what it just configured or created. 2 causes; close BOTH before submitting.

=== Cause 1 of 2: DescribeAccountConfiguration answers 200 without accountConfiguration.kmsKeyId after an update set one (fixture medialive-account-configuration-roundtrip, step task#2; same cause at step task#6) ===

AWS: Smithy: DescribeAccountConfiguration — GET /prod/accountConfiguration, success 200, output member AccountConfiguration (jsonName accountConfiguration) targeting the AccountConfiguration structure, whose only member is KmsKeyId (jsonName kmsKeyId), documented "Specifies the KMS key to use for all features that use key encryption. Specify the ARN of a KMS key that you have created. Or leave blank to use the key that MediaLive creates and manages for you". UpdateAccountConfiguration (PUT /prod/accountConfiguration, success 200, same AccountConfiguration in and out) is the only writer of that setting, and its request is documented as the list of account-configuration parameters to update. The reporting rule that follows: once an update has set a customer-managed key for the account in a region, every later DescribeAccountConfiguration against that regional endpoint reports that key verbatim in accountConfiguration.kmsKeyId, until a later update blanks it. Recorded behavior: the roundtrip's two recorded reads each answer 200 with accountConfiguration.kmsKeyId equal to the key ARN the preceding update set (the corpus carries it as the fixture variable baseline_kms_key_id — the ARN value itself is per-run and is NOT part of the requirement; its presence and byte-for-byte equality with what the update stored is).

Emulator: answers 200 with an accountConfiguration object that carries NO kmsKeyId member at all (replay: "$.accountConfiguration.kmsKeyId: missing (expected '{{baseline_kms_key_id}}', got None)"), both on the read right after the update and on the later read in the same fixture. The update itself did not diverge, so the configuration was accepted and echoed, and only the read loses it.

Why: a client that pins a customer-managed KMS key for MediaLive and then reads the account configuration back sees no key, which is the documented encoding of "no customer key — use the one MediaLive manages". Configuration managers and drift detection therefore conclude the write never took effect and re-apply it on every pass, and any code that reads the configured key out of DescribeAccountConfiguration (to grant it, rotate it or pass it on) gets nothing.

Fix: the medialive crate's DescribeAccountConfiguration handler and the account-configuration state projection it reads through — the read must answer from the same per-region record the preceding UpdateAccountConfiguration wrote. Hypothesis: the read and the write do not agree on the per-region key they use (the read resolves a region, or a store, the write did not write), since the shared projection would otherwise report what the update already reported.

Traces: medialive-account-configuration-roundtrip#task#2 and #task#6 (DescribeAccountConfiguration, 200 -> 200). Code to look at: services/service-com-amazonaws-medialive-medialive/src/ops/describe_account_configuration.rs:144-169, services/service-com-amazonaws-medialive-medialive/src/state/account_configuration.rs:100-107 and 133-158, services/service-com-amazonaws-medialive-medialive/src/ops/update_account_configuration.rs:179-207.

=== Cause 2 of 2: ListCloudWatchAlarmTemplateGroups omits the group just created, and reports the service-published groups under fabricated ids/arns and wrong templateCount (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#3) ===

AWS: Smithy: ListCloudWatchAlarmTemplateGroups — GET /prod/cloudwatch-alarm-template-groups, success 200, output cloudWatchAlarmTemplateGroups: __listOfCloudWatchAlarmTemplateGroupSummary, paginated (maxResults/nextToken); CloudWatchAlarmTemplateGroupSummary requires arn, id, name, createdAt and templateCount ("The number of templates in a group"), and Id is documented "A cloudwatch alarm template group's id. AWS provided template groups have ids that start with `aws-`". CreateCloudWatchAlarmTemplateGroup — POST the same collection, success 201, Name required and documented unique within a resource type in a region. The behavior that follows: a group this account created in a region is a member of that region's collection, so the very next unfiltered listing (scope absent, i.e. all scopes) reports it alongside the groups the service publishes. For the published groups themselves the model says only that their ids start with `aws-`; their actual ids, arns and template counts are AWS's own — recorded behavior: the recording reports them as id/arn pairs such as aws-1160891 (AWS-CloudFrontGeneralWorkflows, templateCount 2), aws-1484347 (AWS-MediaLiveUdpAndRtmpWorkflows, templateCount 9), aws-2520023 (AWS-MediaPackageGeneralWorkflows, templateCount 3), aws-2587288, aws-3414886, aws-3840949 ... with arn arn:aws:medialive:<region>::cloudwatch-alarm-template-group:<id> (empty account segment).

Emulator: answers 200, but (a) the group created earlier in the same fixture is MISSING from the listing entirely — the entry with the created name is absent (replay: '$.cloudWatchAlarmTemplateGroups[name="sim2real-probe-medialive-cwgroup-7f786eeb"]: missing (expected {...templateCount 0, description "initial description", tags {}}, got None)'), although the create was accepted and nothing deleted it; and (b) every service-published group is reported under an id and arn this emulator mints in catalog order (aws-1000001 ... aws-1000010, arn arn:aws:medialive:<region>::cloudwatch-alarm-template-group:aws-100000N) instead of AWS's published ids, and with a templateCount derived from the emulator's own seed rows (e.g. 4 where AWS reports 9 for AWS-MediaLiveUdpAndRtmpWorkflows).

Why: (a) a client cannot see a group it has just created, so create-then-list CRUD loops, name-to-id resolution by listing, and paging over the account's own groups all behave as if the create never happened; (b) a client that pins a published group by id or arn — a signal map association, a template create inside a published group, IaC that records the arn — pins a value that exists only in this emulator and resolves to nothing on AWS, and reads a template count that does not match the catalog. Related open issue 20261106T213951Z-medialive-service-published-catalogs-are-not-served-alarm.md covers the catalog-listing fixtures keyed by arn; the replay reports THIS step as unowned, and the missing created group is not part of that issue at all.

Fix: the ListCloudWatchAlarmTemplateGroups handler's filtering/paging and the state read behind it for the missing created group; the published-catalog seed table for the ids, arns and template counts. Hypothesis for (a): the created group is dropped by the listing's region/scope filter or by the ordering/resume key, since the create path stores it and the single-resource reads in the same fixture passed.

Traces: medialive-cloudwatch-alarm-template-group-lifecycle#task#3 (ListCloudWatchAlarmTemplateGroups, 200 -> 200). Code to look at: services/service-com-amazonaws-medialive-medialive/src/ops/list_cloud_watch_alarm_template_groups.rs:147-226, services/service-com-amazonaws-medialive-medialive/src/state/alarm_template_groups.rs:210-254, services/service-com-amazonaws-medialive-medialive/src/state/managed_monitor_templates.rs:96-132.

## Evidence
Cause 1 — fixture medialive-account-configuration-roundtrip, step task#2 (cluster also covers task#6), operation DescribeAccountConfiguration, verdict mismatch, expected status 200, actual status 200.
  Expected: accountConfiguration.kmsKeyId present, equal to the KMS key ARN the preceding UpdateAccountConfiguration stored.
  Actual:   accountConfiguration present, kmsKeyId absent — replay detail: "$.accountConfiguration.kmsKeyId: missing (expected '{{baseline_kms_key_id}}', got None)".
  No status or error-code divergence (200 -> 200); the divergence is a dropped member on the read-back.

Cause 2 — fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#3, operation ListCloudWatchAlarmTemplateGroups, verdict mismatch, expected status 200, actual status 200.
  Expected (recorded): the account's just-created group plus the service-published groups with AWS's ids/arns/template counts, e.g.
    - name sim2real-probe-medialive-cwgroup-7f786eeb, templateCount 0, description "initial description", tags {}
    - name AWS-CloudFrontGeneralWorkflows, id aws-1160891, arn arn:aws:medialive:us-west-2::cloudwatch-alarm-template-group:aws-1160891
    - name AWS-MediaLiveUdpAndRtmpWorkflows, id aws-1484347, templateCount 9
    - name AWS-MediaPackageGeneralWorkflows, id aws-2520023
    - name AWS-MediaConnectCdiJpegxsWorkflows, id aws-2587288
  Actual: the created group is absent ("missing ... got None"); the published groups carry emulator-minted ids/arns and lower template counts, e.g. AWS-CloudFrontGeneralWorkflows -> id aws-1000001 / arn ...:cloudwatch-alarm-template-group:aws-1000001, AWS-MediaLiveUdpAndRtmpWorkflows -> id aws-1000008 with templateCount 4, AWS-MediaPackageGeneralWorkflows -> id aws-1000009, AWS-MediaConnectCdiJpegxsWorkflows -> id aws-1000003.
  The ids, arns and counts above are the recorded catalog's published values; the created group's generated name and id are per-run fixture values and only their PRESENCE in the listing is the requirement.

## Steps to Reproduce
Cause 1 (fixture medialive-account-configuration-roundtrip, step task#2):
1. Boot the emulator from this checkout and point a medialive client at its data listener (region us-west-2).
2. PUT /prod/accountConfiguration (UpdateAccountConfiguration) with accountConfiguration.kmsKeyId set to a KMS key ARN, e.g. arn:aws:kms:us-west-2:000000000000:key/<key-id>. It answers 200 and echoes the key.
3. GET /prod/accountConfiguration (DescribeAccountConfiguration) on the same regional endpoint.
4. Observe 200 with accountConfiguration carrying no kmsKeyId member; AWS reports the key set in step 2. Repeating the read later in the session (step task#6 of the fixture) behaves the same.

Cause 2 (fixture medialive-cloudwatch-alarm-template-group-lifecycle, step task#3):
1. POST /prod/cloudwatch-alarm-template-groups (CreateCloudWatchAlarmTemplateGroup) with a fresh name and a description; it answers 201 with a minted id.
2. GET /prod/cloudwatch-alarm-template-groups (ListCloudWatchAlarmTemplateGroups) with no scope, maxResults or nextToken.
3. Observe 200 whose cloudWatchAlarmTemplateGroups contains no entry whose name is the one created in step 1.
4. In the same response, observe that the service-published groups are reported with ids aws-1000001..aws-1000010 and matching arns rather than AWS's published ids (aws-1160891, aws-1484347, aws-2520023, ...), and that templateCount differs (4 vs 9 for AWS-MediaLiveUdpAndRtmpWorkflows).

Replay both with replay_wire_corpus and confirm medialive-account-configuration-roundtrip#task#2, #task#6 and medialive-cloudwatch-alarm-template-group-lifecycle#task#3 all pass.
