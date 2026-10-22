Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisanalyticsv2-application-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisanalyticsv2-application-lifecycle kinesisanalyticsv2-application-negative-envelopes kinesisanalyticsv2-application-version-history
Issue-Shape-Id: com.amazonaws.kinesisanalyticsv2#KinesisAnalytics_20180523
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->5xx|$.ApplicationDetail
Issue-Key: kinesisanalyticsv2|CreateApplication|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->5xx|$.ApplicationDetail
Issue-Key: kinesisanalyticsv2|DescribeApplication|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->5xx|$.ApplicationDetail
Issue-Key: kinesisanalyticsv2|UpdateApplication|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|DescribeApplicationVersion|2xx->5xx|$.ApplicationVersionDetail
Issue-Key: kinesisanalyticsv2|DescribeApplicationVersion|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|ListApplicationVersions|2xx->5xx|$.ApplicationVersionSummaries
Issue-Key: kinesisanalyticsv2|ListApplicationVersions|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|TagResource|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|ListTagsForResource|2xx->5xx|$.Tags
Issue-Key: kinesisanalyticsv2|ListTagsForResource|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|UntagResource|2xx->5xx|status
Issue-Key: kinesisanalyticsv2|ListApplicationSnapshots|2xx->5xx|$.SnapshotSummaries
Issue-Key: kinesisanalyticsv2|ListApplicationSnapshots|2xx->5xx|status
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# Kinesis Analytics V2 wire lifecycle operations all answer 500 AmbiguousService instead of their modeled 200 result

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
This bundle carries 10 causes. All ten share one fault: every awsJson1_1 request signed for 'kinesisanalytics' is refused by the emulator's service-resolution layer with HTTP 500 and error code 'AmbiguousService' (recorded message, example only: "2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one"), because the 2015-08-14 (kinesisanalytics) and 2018-05-23 (kinesisanalyticsv2) services both declare endpointPrefix/sigv4Name 'kinesisanalytics'. Real AWS serves both API versions behind that one endpoint and signing name and disambiguates them by the awsJson1_1 X-Amz-Target service-shape prefix (KinesisAnalytics_20180523.<Operation> vs KinesisAnalytics_20150814.<Operation>), so a shared signing name is normal and is not an error condition. 'AmbiguousService' is not a modeled error of this service and does not exist in AWS. Because the refusal happens before dispatch, NO operation-level behavior of kinesisanalyticsv2 is exercised at all: each cause below is a separate operation whose own modeled success envelope must be produced once routing dispatches, and each must be re-replayed before this issue is closed. The replay reported state_reset_failures=0, so these are not cross-fixture state artifacts; 4 further steps were blocked downstream of these failures.

An open BEHAVIORAL issue, 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md, reports the same root cause from the behavioral corpus, but it carries only behavioral check ids and no wire fixture/step keys, so it does not gate any of the wire steps below; they are filed here.

Cause 1 of 10: CreateApplication (success envelope).
AWS: Smithy KinesisAnalytics_20180523 (awsJson1_1) CreateApplication -> 200 with REQUIRED output member ApplicationDetail (input requires ApplicationName @length 1..128 @pattern ^[a-zA-Z0-9_.-]+$, RuntimeEnvironment enum, ServiceExecutionRole @length 1..2048 @pattern ^arn:); modeled errors are CodeValidationException/InvalidArgumentException/InvalidRequestException/LimitExceededException/ResourceInUseException/TooManyTagsException/UnsupportedOperationException (400) and ConcurrentModificationException (409).
Emulator: HTTP 500 with __type AmbiguousService; $.ApplicationDetail absent entirely.
Why: a client cannot create a KDA V2 application at all, and the 500/unmodeled code is classified retryable, so SDKs retry to their ceiling and then surface an unknown fault rather than a result or a modeled 400/409.
Fix: the shared request router / service-resolution layer (dispatch awsJson1_1 on the X-Amz-Target service-shape prefix, never refuse a signing name legitimately shared across API versions), then the CreateApplication handler and its state so the created application is recorded and serialized as ApplicationDetail.
Traces: kinesisanalyticsv2-application-lifecycle#task#2.

Cause 2 of 10: DescribeApplication (success envelope).
AWS: Smithy DescribeApplication -> 200 with REQUIRED ApplicationDetail; errors InvalidArgumentException/InvalidRequestException/ResourceNotFoundException (400).
Emulator: HTTP 500 AmbiguousService; $.ApplicationDetail missing (the fixture expects the full created detail including ApplicationStatus, ApplicationVersionId and the configuration descriptions).
Why: callers cannot read back application state, so no poll-until-READY or configuration-inspection flow works.
Fix: router as in Cause 1, then the DescribeApplication handler/serializer reading the stored application.
Traces: kinesisanalyticsv2-application-lifecycle#task#3 (cluster also covers kinesisanalyticsv2-application-negative-envelopes#task#11, kinesisanalyticsv2-application-version-history#task#3 and #task#9).

Cause 3 of 10: UpdateApplication (success envelope and version bump).
AWS: Smithy UpdateApplication -> 200 with REQUIRED ApplicationDetail (+optional OperationId); CurrentApplicationVersionId @range 1..999999999; errors include ConcurrentModificationException (409) and InvalidApplicationConfigurationException/ResourceNotFoundException (400).
Emulator: HTTP 500 AmbiguousService; $.ApplicationDetail missing. The recording shows a successful update must return the NEXT ApplicationVersionId with ApplicationVersionUpdatedFrom set to the prior one and the updated configuration reflected (recorded example: version 2 updated from 1, ParallelismConfigurationDescription ConfigurationType CUSTOM).
Why: optimistic-concurrency clients read CurrentApplicationVersionId from the update response; with no response they cannot chain updates.
Fix: router as in Cause 1, then the UpdateApplication handler and application state (version increment + ApplicationVersionUpdatedFrom).
Traces: kinesisanalyticsv2-application-lifecycle#task#4 (cluster also covers kinesisanalyticsv2-application-version-history#task#4 and #task#8).

Cause 4 of 10: DescribeApplicationVersion (success envelope).
AWS: Smithy DescribeApplicationVersion -> 200 with ApplicationVersionDetail (ApplicationDetail shape); input ApplicationVersionId required, @range 1..999999999; errors InvalidArgumentException/ResourceNotFoundException/UnsupportedOperationException (400).
Emulator: HTTP 500 AmbiguousService; $.ApplicationVersionDetail missing.
Why: historical versions are unreadable, so rollback/compare flows are impossible.
Fix: router as in Cause 1, then the DescribeApplicationVersion handler plus per-version history retention in application state.
Traces: kinesisanalyticsv2-application-lifecycle#task#5 (cluster also covers kinesisanalyticsv2-application-version-history#task#5, #task#6 and #task#10).

Cause 5 of 10: ListApplicationVersions (success envelope).
AWS: Smithy ListApplicationVersions -> 200 with ApplicationVersionSummaries (+NextToken), paginated (inputToken/outputToken NextToken, items ApplicationVersionSummaries, pageSize Limit @range 1..50).
Emulator: HTTP 500 AmbiguousService; $.ApplicationVersionSummaries missing (recorded example: one summary per retained version with ApplicationVersionId and ApplicationStatus).
Why: paginating clients get a fault instead of a list, and the empty-vs-populated distinction is unavailable.
Fix: router as in Cause 1, then the ListApplicationVersions handler/serializer over the version history in state.
Traces: kinesisanalyticsv2-application-lifecycle#task#6 (cluster also covers kinesisanalyticsv2-application-version-history#task#7).

Cause 6 of 10: TagResource (empty success envelope).
AWS: Smithy TagResource -> 200 with an EMPTY output structure; input ResourceARN required (@length 1..2048, @pattern ^arn:) and Tags required (@length 1..200); errors ConcurrentModificationException (409), InvalidArgumentException/ResourceInUseException/ResourceNotFoundException/TooManyTagsException (400).
Emulator: HTTP 500 AmbiguousService instead of 200.
Why: tagging appears to fail, so tag-then-verify provisioning breaks.
Fix: router as in Cause 1, then the TagResource handler and the tag store keyed by application ARN.
Traces: kinesisanalyticsv2-application-lifecycle#task#7.

Cause 7 of 10: ListTagsForResource (success envelope).
AWS: Smithy ListTagsForResource -> 200 with Tags (@length 1..200); errors ConcurrentModificationException (409), InvalidArgumentException/ResourceNotFoundException (400).
Emulator: HTTP 500 AmbiguousService; $.Tags missing (recorded example: the single tag written by TagResource).
Why: tags written earlier are unreadable, so tag-based lookup and the post-UntagResource re-read both fail.
Fix: router as in Cause 1, then the ListTagsForResource handler reading the tag store.
Traces: kinesisanalyticsv2-application-lifecycle#task#8 (cluster also covers kinesisanalyticsv2-application-lifecycle#task#10, the re-read after untagging).

Cause 8 of 10: UntagResource (empty success envelope).
AWS: Smithy UntagResource -> 200 with an EMPTY output structure; input ResourceARN and TagKeys required; same error set as TagResource.
Emulator: HTTP 500 AmbiguousService instead of 200.
Why: tag removal cannot be performed or confirmed.
Fix: router as in Cause 1, then the UntagResource handler mutating the tag store.
Traces: kinesisanalyticsv2-application-lifecycle#task#9.

Cause 9 of 10: ListApplicationSnapshots (success envelope, including the empty list).
AWS: Smithy ListApplicationSnapshots -> 200 with SnapshotSummaries (+NextToken), paginated; errors InvalidArgumentException/UnsupportedOperationException (400).
Emulator: HTTP 500 AmbiguousService; $.SnapshotSummaries missing where the recording expects a present but EMPTY list.
Why: a client cannot distinguish 'no snapshots' from a server fault; an absent list also breaks SDK pagination which expects the member to exist.
Fix: router as in Cause 1, then the ListApplicationSnapshots handler/serializer, which must emit the member even when no snapshots exist.
Traces: kinesisanalyticsv2-application-lifecycle#task#11.

Cause 10 of 10: CreateApplication in fixture setup — the same refusal blocks fixture bring-up, not just asserted tasks.
AWS: as Cause 1, 200 with required ApplicationDetail.
Emulator: HTTP 500 AmbiguousService on the setup-phase create, so the negative-envelope and version-history fixtures run their task phases with no application present.
Why: every later step of those fixtures is evaluated against a non-existent resource, which both hides real per-operation defects and makes the corpus unreplayable end to end.
Fix: router as in Cause 1 (no separate handler work beyond Cause 1); listed separately because it is a distinct replay key that must pass.
Traces: kinesisanalyticsv2-application-negative-envelopes#setup#2 (cluster also covers kinesisanalyticsv2-application-version-history#setup#2).

## Evidence
Replay of trace_set simparity-wire-aws-v1 against the emulator built from mainline: 38 steps total, 6 passed, 28 mismatched, 4 blocked, score 15.8%, state_reset_failures=0, emulator_up=true (/__admin/health 200).

Expected (AWS, Smithy KinesisAnalytics_20180523, awsJson1_1): HTTP 200 for each of CreateApplication (required ApplicationDetail), DescribeApplication (required ApplicationDetail), UpdateApplication (required ApplicationDetail), DescribeApplicationVersion (ApplicationVersionDetail), ListApplicationVersions (ApplicationVersionSummaries), TagResource (empty body), ListTagsForResource (Tags), UntagResource (empty body), ListApplicationSnapshots (SnapshotSummaries, present even when empty).

Actual (emulator): HTTP 500 on every one of them, body __type 'AmbiguousService' with the recorded message "2 emulated services all claim requests for 'kinesisanalytics': kinesisanalytics, kinesisanalytics; the request is rejected rather than served by an arbitrary one" (message is an example; the requirement is the status and the error code). All asserted output members are absent:
- kinesisanalyticsv2-application-lifecycle#task#2 CreateApplication: status 200->500, $.ApplicationDetail missing.
- kinesisanalyticsv2-application-lifecycle#task#3 DescribeApplication: status 200->500, $.ApplicationDetail missing (4 affected steps).
- kinesisanalyticsv2-application-lifecycle#task#4 UpdateApplication: status 200->500, $.ApplicationDetail missing (3 affected steps).
- kinesisanalyticsv2-application-lifecycle#task#5 DescribeApplicationVersion: status 200->500, $.ApplicationVersionDetail missing (4 affected steps).
- kinesisanalyticsv2-application-lifecycle#task#6 ListApplicationVersions: status 200->500, $.ApplicationVersionSummaries missing, expected [{ApplicationVersionId:1,ApplicationStatus:READY},{ApplicationVersionId:2,ApplicationStatus:READY}] (2 affected steps).
- kinesisanalyticsv2-application-lifecycle#task#7 TagResource: status 200->500.
- kinesisanalyticsv2-application-lifecycle#task#8 ListTagsForResource: status 200->500, $.Tags missing (2 affected steps).
- kinesisanalyticsv2-application-lifecycle#task#9 UntagResource: status 200->500.
- kinesisanalyticsv2-application-lifecycle#task#11 ListApplicationSnapshots: status 200->500, $.SnapshotSummaries missing where [] was expected.
- kinesisanalyticsv2-application-negative-envelopes#setup#2 CreateApplication: status 200->500 (2 affected steps, also the version-history fixture setup).

Model evidence: services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/smithy-model/com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523.json (endpointPrefix/sigv4Name 'kinesisanalytics') vs services/service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814/... (identical endpointPrefix/sigv4Name). Both are in the resolved bootstrap set, which is why two services claim the name.

## Steps to Reproduce
1. Clone mainline and boot the emulator built from it (health: /__admin/health answers 200).
2. Replay the wire corpus simparity-wire-aws-v1, or drive it by hand: client = boto3.client('kinesisanalyticsv2', endpoint_url=<emulator>).
3. Call client.create_application(ApplicationName='probe-app', RuntimeEnvironment='FLINK-1_15', ServiceExecutionRole='arn:aws:iam::000000000000:role/probe-role', ApplicationConfiguration={...}). Observe HTTP 500 with __type AmbiguousService instead of 200 + ApplicationDetail.
4. Repeat with describe_application, update_application, describe_application_version, list_application_versions, tag_resource, list_tags_for_resource, untag_resource and list_application_snapshots — each returns the same 500 AmbiguousService instead of its modeled 200 envelope.
5. To confirm a fix, re-replay ALL ten keys: kinesisanalyticsv2-application-lifecycle#task#2 (representative), plus #task#3, #task#4, #task#5, #task#6, #task#7, #task#8, #task#9, #task#11 and kinesisanalyticsv2-application-negative-envelopes#setup#2. Routing alone unblocks dispatch; each operation's own 200 envelope must then match, so all ten keys must pass before closing.
