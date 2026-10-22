Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-config-lifecycle-tracking-and-downlink
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-config-lifecycle-tracking-and-downlink groundstation-config-other-capability-types groundstation-contact-read-and-error-paths groundstation-mission-profile-error-paths
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|GetConfig|4xx->4xx|$.message
Issue-Key: groundstation|CreateConfig|4xx->4xx|$.message
Issue-Key: groundstation|DescribeContact|4xx->4xx|$.message
Issue-Key: groundstation|CancelContact|4xx->4xx|$.message
Issue-Key: groundstation|ReserveContact|4xx->4xx|$.message
Issue-Key: groundstation|CreateMissionProfile|4xx->4xx|$.message
Issue-Key: groundstation|GetMissionProfile|4xx->4xx|$.message
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# Ground Station not-found errors use one generic template instead of the per-operation AWS text

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfig

## Description
Every Ground Station ResourceNotFoundException in the corpus carries the correct HTTP 434 and the correct x-amzn-errortype, but the `message` is rebuilt from a single emulator-internal template ('No <thing> with id <id> exists' / 'No <type> <thing> exists with ARN <arn>') rather than the per-operation text the service emits. The status and error code are already right in all seven causes — only the message rule diverges — so this is a presentation-fidelity issue, not a control-flow one. Seven causes, one shared root cause: the shared not-found message builder in the groundstation error layer.

Shared AWS basis: Smithy ResourceNotFoundException is @error client, @httpError 434, with a single `message` member and doc 'Resource was not found.' The model does not state the text, so each cause's text below is recorded behavior only; what it shows is that Ground Station uses a DIFFERENT template per operation, some built from the request (id / type / ARN / account) and some a fixed literal. The emulator uses one template for all of them.

Cause 1 of 7: GetConfig not-found message. AWS (recorded): 'No config found with id <configId> and type <configType>' — built from the two @httpLabel path members of GET /config/{configType}/{configId}. Emulator: 'No config of type <configType> with id <configId> exists'. Why: callers and log scrapers that key on AWS's phrasing (and the id-then-type ordering) do not match. Fix: GetConfig's not-found branch / the shared message builder.

Cause 2 of 7: CreateConfig referenced-config not-found message. AWS (recorded): 'Unable to find AntennaUplink Config with ARN: <configArn>' — built from the ARN in the request's configData. Emulator: 'No antenna-uplink config exists with ARN <configArn>'. Note AWS uses the CamelCase capability name ('AntennaUplink Config'), not the wire enum token 'antenna-uplink' from ConfigCapabilityType. Why: the emulator leaks the wire enum spelling into user-facing text. Fix: CreateConfig's reference-resolution error path.

Cause 3 of 7: DescribeContact not-found message. AWS (recorded): the fixed literal 'Contact not found' — no id interpolated. Emulator: 'No contact with id <contactId> exists'. Why: the emulator discloses the requested id where AWS deliberately does not. Fix: DescribeContact's not-found branch.

Cause 4 of 7: CancelContact not-found message. AWS (recorded): "Contact with id '<contactId>' does not exist." — id quoted, sentence terminated with a period. Emulator: 'No contact with id <contactId> exists'. Note this differs from DescribeContact's text for the same missing resource, so the two cannot share one builder. Why: as above. Fix: CancelContact's not-found branch.

Cause 5 of 7: ReserveContact mission-profile not-found message. AWS (recorded): the fixed literal 'MissionProfile not found'. Emulator: 'No mission profile exists with ARN <missionProfileArn>'. Why: AWS does not echo the ARN here; the emulator does. Fix: ReserveContact's missionProfileArn resolution error path.

Cause 6 of 7: CreateMissionProfile tracking-config not-found message. AWS (recorded): 'No config found with ID <configId> and config type: <configType> for customer <accountId>' — built from the trackingConfigArn in the request plus the caller's account. Emulator: 'No tracking config exists with ARN <configArn>'. Why: AWS identifies the missing config by id + type + account; the emulator echoes a raw ARN. Fix: CreateMissionProfile's config-reference resolution error path.

Cause 7 of 7: GetMissionProfile not-found message. AWS (recorded): the fixed literal 'MissionProfile not found' — matching ReserveContact's text and confirming it is the service's mission-profile literal. Emulator: 'No mission profile with id <missionProfileId> exists'. Why: as above. Fix: GetMissionProfile's not-found branch.

Traces (one representative per cause):
- Cause 1: groundstation-config-lifecycle-tracking-and-downlink#task#7 (GetConfig, 434 vs 434)
- Cause 2: groundstation-config-other-capability-types#task#7 (CreateConfig, 434 vs 434)
- Cause 3: groundstation-contact-read-and-error-paths#task#2 (DescribeContact, 434 vs 434)
- Cause 4: groundstation-contact-read-and-error-paths#task#6 (CancelContact, 434 vs 434)
- Cause 5: groundstation-contact-read-and-error-paths#task#7 (ReserveContact, 434 vs 434)
- Cause 6: groundstation-mission-profile-error-paths#task#1 (CreateMissionProfile, 434 vs 434)
- Cause 7: groundstation-mission-profile-error-paths#task#2 (GetMissionProfile, 434 vs 434)

## Evidence
All seven causes matched on status (434) and on x-amzn-errortype; only $.message diverged.

Cause 1 — groundstation-config-lifecycle-tracking-and-downlink#task#7, GetConfig:
  expected 'No config found with id 8fd5911e-2416-42b1-aad9-cae2b28f1230 and type tracking'
  got      'No config of type tracking with id 8fd5911e-2416-42b1-aad9-cae2b28f1230 exists'

Cause 2 — groundstation-config-other-capability-types#task#7, CreateConfig:
  expected 'Unable to find AntennaUplink Config with ARN: arn:aws:groundstation:us-west-2:000000000000:config/antenna-uplink/ed8f59eb-42e2-4bd7-9ab6-47d933490dcd'
  got      'No antenna-uplink config exists with ARN arn:aws:groundstation:us-west-2:000000000000:config/antenna-uplink/ed8f59eb-42e2-4bd7-9ab6-47d933490dcd'

Cause 3 — groundstation-contact-read-and-error-paths#task#2, DescribeContact:
  expected 'Contact not found'
  got      'No contact with id 91d8263b-81bd-4215-8e3b-10e64dde96b4 exists'

Cause 4 — groundstation-contact-read-and-error-paths#task#6, CancelContact:
  expected "Contact with id '91d8263b-81bd-4215-8e3b-10e64dde96b4' does not exist."
  got      'No contact with id 91d8263b-81bd-4215-8e3b-10e64dde96b4 exists'

Cause 5 — groundstation-contact-read-and-error-paths#task#7, ReserveContact:
  expected 'MissionProfile not found'
  got      'No mission profile exists with ARN arn:aws:groundstation:us-west-2:000000000000:mission-profile/70b2aeff-4705-4d03-9796-3c0b5744e5b9'

Cause 6 — groundstation-mission-profile-error-paths#task#1, CreateMissionProfile:
  expected 'No config found with ID ca8a3ddd-1fef-4d20-92f4-347bdca2ae43 and config type: tracking for customer 000000000000'
  got      'No tracking config exists with ARN arn:aws:groundstation:us-west-2:000000000000:config/tracking/ca8a3ddd-1fef-4d20-92f4-347bdca2ae43'

Cause 7 — groundstation-mission-profile-error-paths#task#2, GetMissionProfile:
  expected 'MissionProfile not found'
  got      'No mission profile with id c3e1bd40-8ecd-4f3f-82ee-f66ead24c5c9 exists'

Smithy citation: ResourceNotFoundException = @error('client') @httpError(434), members {message: String}, doc 'Resource was not found.'; bound as an error on GetConfig, CreateConfig, DescribeContact, CancelContact, ReserveContact, CreateMissionProfile and GetMissionProfile. ConfigCapabilityType values include 'tracking' and 'antenna-uplink'.

## Steps to Reproduce
1. Replay the wire corpus: replay_wire_corpus().
2. Cause 1: fixture groundstation-config-lifecycle-tracking-and-downlink, step task#7 — GET /config/tracking/{unknown-uuid} and compare $.message.
3. Cause 2: fixture groundstation-config-other-capability-types, step task#7 — POST /config with a configData that references a nonexistent antenna-uplink config ARN; compare $.message.
4. Cause 3: fixture groundstation-contact-read-and-error-paths, step task#2 — GET /contact/{unknown-uuid}; AWS returns the bare literal 'Contact not found'.
5. Cause 4: same fixture, step task#6 — DELETE /contact/{unknown-uuid} (CancelContact); AWS quotes the id and ends with a period.
6. Cause 5: same fixture, step task#7 — POST /contact (ReserveContact) with a nonexistent missionProfileArn; AWS returns 'MissionProfile not found' without echoing the ARN.
7. Cause 6: fixture groundstation-mission-profile-error-paths, step task#1 — POST /missionprofile with a nonexistent trackingConfigArn; AWS names id, config type and customer account.
8. Cause 7: same fixture, step task#2 — GET /missionprofile/{unknown-uuid}; AWS returns 'MissionProfile not found'.
In every case the status (434) and x-amzn-errortype already match; only the message body differs.
