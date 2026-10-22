Issue-Source: trace-sweep-divergence
Issue-Trace: route53-create-hosted-zone-duplicate-caller-reference-error
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-create-hosted-zone-duplicate-caller-reference-error route53-dnssec-status-guardrails route53-error-not-found-and-invalid-vpc route53-hosted-zone-accelerated-recovery-toggle route53-public-zone-vpc-association-guardrails route53-test-dns-answer-with-record-lifecycle route53-vpc-association-authorization-lifecycle route53-hosted-zone-create-verify-delete route53-public-hosted-zone-record-lifecycle route53-tags-for-resource-hosted-zone
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateHostedZone|2xx->2xx|$.CreateHostedZoneResponse.HostedZone.Config
Issue-Key: route53|GetHostedZone|2xx->2xx|$.GetHostedZoneResponse.HostedZone.Config
Issue-Key: route53|GetHostedZone|2xx->2xx|$.GetHostedZoneResponse.HostedZoneDeletionProtectionEnabled
Issue-Key: route53|GetHostedZone|2xx->2xx|$.GetHostedZoneResponse.HostedZone.Config.Comment
Issue-Key: route53|ListTagsForResource|2xx->2xx|$.ListTagsForResourceResponse.ResourceTagSet.Tags
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# Route 53 hosted-zone reads omit modeled response members and lose comment updates

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHostedZone

## Description
Route 53 hosted-zone success bodies are serialized with modeled/recorded members dropped, so a client reading a zone back cannot see its configuration, its deletion-protection flag, or its (empty) tag set, and cannot observe a comment update. FIVE DISTINCT CAUSES are bundled here; all five must be closed.

Cause 1 of 5: CreateHostedZone omits HostedZone.Config for a public zone.
AWS: Smithy com.amazonaws.route53#HostedZone has member Config -> HostedZoneConfig, whose PrivateZone member is a boolean with @default false. The recorded 201 CreateHostedZoneResponse carries HostedZone/Config with PrivateZone=false even when the request sent no HostedZoneConfig (recorded behavior for the PrivateZone element; the model's Config doc only says Config/Comment are omitted when HostedZoneConfig AND Comment were omitted from the request).
Emulator: answers 201 (status matches) but the body has no $.CreateHostedZoneResponse.HostedZone.Config element at all.
Why: a client that reads HostedZone.Config.PrivateZone to decide whether a zone is private gets a missing element instead of 'false', and SDK models that treat Config as always-present on a create response see a null sub-structure.
Fix: the zone -> HostedZone projection used by the CreateHostedZone handler/serializer must always emit Config with PrivateZone (defaulting to false) rather than only when a comment or the private flag is set.
Traces: route53-create-hosted-zone-duplicate-caller-reference-error#setup#1 (and 6 further fixtures in the same cluster: route53-dnssec-status-guardrails#task#1, route53-error-not-found-and-invalid-vpc#task#1, route53-hosted-zone-accelerated-recovery-toggle#setup#1, route53-public-zone-vpc-association-guardrails#setup#2, route53-test-dns-answer-with-record-lifecycle#task#1, route53-vpc-association-authorization-lifecycle#setup#1). Projection cited at services/service-com-amazonaws-route53-awsdnsv20130401/src/state.rs:216-230.

Cause 2 of 5: GetHostedZone omits BOTH HostedZone.Config and HostedZoneDeletionProtectionEnabled.
AWS: same HostedZone.Config member as above; for HostedZoneDeletionProtectionEnabled the checked-in model declares no such member on GetHostedZoneResponse, so recorded behavior only - the recording shows a 200 GetHostedZoneResponse carrying HostedZoneDeletionProtectionEnabled=false alongside HostedZone/Config/PrivateZone=false.
Emulator: 200 with both $.GetHostedZoneResponse.HostedZone.Config and $.GetHostedZoneResponse.HostedZoneDeletionProtectionEnabled absent.
Why: a client cannot tell a public zone from a private one, and cannot read deletion protection at all, from a successful read of the zone.
Fix: the GetHostedZone handler/serializer and the shared HostedZone projection must emit Config unconditionally and add the deletion-protection flag (false unless set) to the response.
Traces: route53-create-hosted-zone-duplicate-caller-reference-error#task#3, route53-hosted-zone-accelerated-recovery-toggle#task#4. Handler at services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_hosted_zone.rs:159-165.

Cause 3 of 5: GetHostedZone omits HostedZoneDeletionProtectionEnabled on a zone that DOES carry a Config.
AWS: recorded behavior only - the 200 GetHostedZoneResponse includes HostedZoneDeletionProtectionEnabled=false.
Emulator: 200, element absent, while the rest of the body matches.
Why: same client consequence as cause 2, on the common create-verify-delete path; it is a separate code path because the Config element is present here, so fixing cause 1/2's Config emission alone does not close it.
Fix: the GetHostedZone output construction must carry the deletion-protection flag independently of whether Config is populated.
Traces: route53-hosted-zone-create-verify-delete#task#3.

Cause 4 of 5: a hosted zone's Comment never changes, so GetHostedZone keeps returning the pre-update comment.
AWS: Smithy UpdateHostedZoneComment (POST /2013-04-01/hostedzone/{Id}) takes Comment (ResourceDescription, len 0..256) and returns the updated HostedZone; a subsequent GetHostedZone reports HostedZone.Config.Comment as the new value. The recording expects 'updated comment' after the update call.
Emulator: GetHostedZone answers 200 with Config.Comment still at the original value ('initial comment' in the recording), and additionally omits HostedZoneDeletionProtectionEnabled. Root cause: UpdateHostedZoneComment is still a scaffold that answers 501 (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/update_hosted_zone_comment.rs:145-163), so no write ever reaches StoredZone.comment. The scaffold itself has its own stub-owned issue; this entry is the GetHostedZone read divergence the replay reported as unowned, and it is only closed once the comment write lands in state and is read back.
Why: a client that updates a zone comment and reads it back sees the stale comment and must conclude the update silently did nothing.
Fix: implement the comment write in the UpdateHostedZoneComment handler against the shared zone store so the GetHostedZone read reflects it.
Traces: route53-public-hosted-zone-record-lifecycle#task#9.

Cause 5 of 5: ListTagsForResource drops ResourceTagSet.Tags for a resource with no tags.
AWS: Smithy ResourceTagSet has members ResourceType, ResourceId and Tags -> TagList (@length min 1, max 10). The recorded 200 ListTagsForResourceResponse for an untagged hosted zone still carries an EMPTY ResourceTagSet/Tags element (expected value '' in the replay), i.e. the container element is present with no Tag children.
Emulator: 200 with $.ListTagsForResourceResponse.ResourceTagSet.Tags absent entirely - the handler maps an empty tag map to None (services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_tags_for_resource.rs:175-183 via state.rs:421-430).
Why: XML clients that walk ResourceTagSet/Tags to enumerate tags hit a missing node rather than an empty list, which deserializes to null instead of an empty collection and breaks tag round-trips after every tag is removed.
Fix: the ListTagsForResource serializer/projection must emit an empty Tags container when the resource holds no tags, instead of omitting the member.
Traces: route53-tags-for-resource-hosted-zone#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0).

Cause 1 - CreateHostedZone, expected 201 / actual 201: '$.CreateHostedZoneResponse.HostedZone.Config: missing (expected {\'PrivateZone\': \'false\'}, got None)' (cluster of 7 steps).
Cause 2 - GetHostedZone, expected 200 / actual 200: '$.GetHostedZoneResponse.HostedZone.Config: missing (expected {\'PrivateZone\': \'false\'}, got None)' and '$.GetHostedZoneResponse.HostedZoneDeletionProtectionEnabled: missing (expected \'false\', got None)' (cluster of 2 steps).
Cause 3 - GetHostedZone, expected 200 / actual 200: '$.GetHostedZoneResponse.HostedZoneDeletionProtectionEnabled: missing (expected \'false\', got None)'.
Cause 4 - GetHostedZone, expected 200 / actual 200: '$.GetHostedZoneResponse.HostedZone.Config.Comment: value_mismatch (expected \'updated comment\', got \'initial comment\')' plus the missing deletion-protection flag.
Cause 5 - ListTagsForResource, expected 200 / actual 200: '$.ListTagsForResourceResponse.ResourceTagSet.Tags: missing (expected \'\', got None)'.

Model evidence: smithy_query(shape=HostedZone,HostedZoneConfig,ResourceTagSet) on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json - HostedZone.Config -> HostedZoneConfig{Comment, PrivateZone @default false}; ResourceTagSet.Tags -> TagList @length 1..10. GetHostedZoneResponse declares no HostedZoneDeletionProtectionEnabled member, so that element is grounded in the recording only.

## Steps to Reproduce
1. Replay fixture route53-create-hosted-zone-duplicate-caller-reference-error, step setup#1 (POST /2013-04-01/hostedzone for a public zone) and inspect the 201 body for HostedZone/Config/PrivateZone.
2. Replay route53-create-hosted-zone-duplicate-caller-reference-error#task#3 and route53-hosted-zone-create-verify-delete#task#3 (GET /2013-04-01/hostedzone/{Id}) and inspect the 200 body for HostedZone/Config and HostedZoneDeletionProtectionEnabled.
3. Replay route53-public-hosted-zone-record-lifecycle#task#9: create a zone with comment 'initial comment', POST UpdateHostedZoneComment with 'updated comment', then GET the zone - Config/Comment is still the original value.
4. Replay route53-tags-for-resource-hosted-zone#task#5 (GET /2013-04-01/tags/hostedzone/{Id} for an untagged zone) and inspect the 200 body for the ResourceTagSet/Tags container.
