Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-domain-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-domain-lifecycle connecthealth-subscription-error-paths connecthealth-subscription-lifecycle
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|CreateDomain|2xx->2xx|$.accountId
Issue-Key: connecthealth|CreateDomain|2xx->2xx|$.onboardingGeneration
Issue-Key: connecthealth|GetDomain|2xx->2xx|$.accountId
Issue-Key: connecthealth|GetDomain|2xx->2xx|$.onboardingGeneration
Issue-Key: connecthealth|GetDomain|2xx->2xx|$.tags
Issue-Key: connecthealth|DeleteDomain|2xx->2xx|$.deletedAt
Issue-Key: connecthealth|DeleteDomain|2xx->2xx|$.status
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# Domain responses omit accountId onboardingGeneration tags and deletion fields

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
The ConnectHealth Domain handlers build their responses from an incomplete projection of domain state: the account-scoping and lifecycle fields the recorded service returns are never serialized, and DeleteDomain reports a non-terminal status. This bundle carries 5 causes; all 5 must be closed.

Cause 1 of 5 - CreateDomain response omits accountId and onboardingGeneration (fixture connecthealth-domain-lifecycle, step setup#1).
AWS: recorded behavior only - Smithy CreateDomainOutput models domainId/arn/name/kmsKeyArn/encryptionContext/status/webAppUrl/webAppConfiguration/createdAt and does NOT model accountId or onboardingGeneration, so the model says nothing about them. The recording shows the 201 CreateDomain body additionally carrying a top-level accountId (the 12-digit owning account of the new domain) and a top-level onboardingGeneration discriminator (recorded value 'LEGACY').
Emulator: answers 201 with the correct status line but both keys absent from the JSON body ($.accountId missing, $.onboardingGeneration missing).
Why: a client that keys cached domain records by owning account, or that branches on the onboarding generation to decide which web-app/provisioning path a domain uses, reads null for both and either mis-attributes the domain or silently takes the wrong branch. Because the status is a correct 201, nothing signals the truncation.
Fix: the CreateDomain response serializer - it must project the domain's owning account and onboarding generation out of state onto the output, not only the modeled members.

Cause 2 of 5 - GetDomain response omits accountId, onboardingGeneration and tags (fixture connecthealth-domain-lifecycle, step task#2).
AWS: Smithy GetDomainOutput models member tags -> TagMap (map<TagKey,TagValue>, documented 'Tags associated with the Domain'); accountId and onboardingGeneration are recorded behavior only, same as Cause 1. The recording shows GetDomain returning all three, with tags serialized as an empty object when the domain carries no tags.
Emulator: answers 200 with $.accountId, $.onboardingGeneration and $.tags all absent.
Why: GetDomain is the read-back path, so this is the shape every describe/refresh loop sees. An absent tags map is not the same as an empty one - a client that iterates response.tags gets a null dereference rather than an empty iteration, and tag-reconciliation logic reads 'unknown' where AWS says 'none'.
Fix: the GetDomain response serializer - emit tags as an empty map rather than omitting it when the domain has no tags, and project accountId/onboardingGeneration as in Cause 1.

Cause 3 of 5 - DeleteDomain response omits deletedAt (fixture connecthealth-domain-lifecycle, step task#6).
AWS: recorded behavior only - Smithy DeleteDomainOutput models only domainId, arn and status (DomainStatus enum ACTIVE|DELETING|DELETED). The recording shows the 200 DeleteDomain body additionally carrying a deletedAt timestamp marking when deletion was recorded; the fixture asserts its presence and shape, not a literal value.
Emulator: answers 200 with $.deletedAt absent.
Why: a client that stamps its own deletion audit record from the response timestamp has no value to read and must substitute local clock time, so deletion ordering recorded against the emulator does not match what AWS reports.
Fix: the DeleteDomain handler/serializer - set and emit the deletion timestamp when it transitions the domain.

Cause 4 of 5 - DeleteDomain omits deletedAt AND returns status DELETING where the recording shows DELETED (fixture connecthealth-subscription-error-paths, step teardown#5).
AWS: Smithy DeleteDomainOutput.status -> DomainStatus, enum values ACTIVE, DELETING, DELETED; deletedAt is recorded behavior only as in Cause 3. The recording shows this DeleteDomain call returning status 'DELETED' together with a deletedAt timestamp - i.e. deletion of this domain completes synchronously and the response reports the terminal state.
Emulator: answers 200 with $.status = 'DELETING' and $.deletedAt absent.
Why: DELETING is a non-terminal state, so a client polling GetDomain until the domain reaches DELETED keeps polling against an emulator that already considers the work done, or treats the teardown as still in flight and refuses to release the dependent resources. Note the related Cause 3 step returns the status the fixture expected, so the status defect is specific to the completion path, not to every DeleteDomain.
Hypothesis: the handler emits a fixed DELETING literal instead of the status it just wrote to state.
Fix: the DeleteDomain handler's state transition - report the domain's actual post-transition status, and emit deletedAt with it.

Cause 5 of 5 - CreateDomain response omits accountId (fixture connecthealth-subscription-lifecycle, step setup#1).
AWS: as Cause 1 - recorded behavior only; the recorded 201 body carries a top-level accountId for the owning account.
Emulator: answers 201 with $.accountId absent. This is the same serializer gap as Cause 1 reached through a different fixture's setup, and is listed separately so the fixer re-replays it: it is the step that gates every subsequent subscription step in connecthealth-subscription-lifecycle.
Why: as Cause 1.
Fix: as Cause 1.

Traces: connecthealth-domain-lifecycle#setup#1 (CreateDomain), connecthealth-domain-lifecycle#task#2 (GetDomain), connecthealth-domain-lifecycle#task#6 (DeleteDomain), connecthealth-subscription-error-paths#teardown#5 (DeleteDomain, also connecthealth-subscription-lifecycle#teardown#9), connecthealth-subscription-lifecycle#setup#1 (CreateDomain).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 15/43 steps passed, score 34.9%).

Cause 1 - connecthealth-domain-lifecycle#setup#1, CreateDomain, expected 201 / actual 201:
  $.accountId: missing (expected '000000000000', got None)
  $.onboardingGeneration: missing (expected 'LEGACY', got None)

Cause 2 - connecthealth-domain-lifecycle#task#2, GetDomain, expected 200 / actual 200:
  $.accountId: missing (expected '000000000000', got None)
  $.onboardingGeneration: missing (expected 'LEGACY', got None)
  $.tags: missing (expected {}, got None)

Cause 3 - connecthealth-domain-lifecycle#task#6, DeleteDomain, expected 200 / actual 200:
  $.deletedAt: missing (expected '<shaped-not-asserted>', got None)

Cause 4 - connecthealth-subscription-error-paths#teardown#5, DeleteDomain, expected 200 / actual 200 (cluster also covers connecthealth-subscription-lifecycle#teardown#9):
  $.deletedAt: missing (expected 1788661971.207, got None)
  $.status: value_mismatch (expected 'DELETED', got 'DELETING')

Cause 5 - connecthealth-subscription-lifecycle#setup#1, CreateDomain, expected 201 / actual 201:
  $.accountId: missing (expected '000000000000', got None)

The recorded account id '000000000000' and the timestamp 1788661971.207 are examples from the recording, not requirements; the requirement is that the members are present and carry the domain's own account and deletion time. The diverging status value 'DELETED' vs 'DELETING' in Cause 4 IS a requirement.

Model citations: Smithy GetDomainOutput.tags -> TagMap; Smithy DeleteDomainOutput.status -> DomainStatus enum [ACTIVE, DELETING, DELETED]; CreateDomainOutput/DeleteDomainOutput do not model accountId, onboardingGeneration or deletedAt (recorded behavior only).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture connecthealth-domain-lifecycle, step setup#1 - POST /domain; inspect the 201 body for accountId and onboardingGeneration.
4. Cause 2: same fixture, step task#2 - GET /domain/{domainId} on the domain just created; inspect the 200 body for accountId, onboardingGeneration and tags (expect tags == {} for an untagged domain).
5. Cause 3: same fixture, step task#6 - DELETE /domain/{domainId}; inspect the 200 body for deletedAt.
6. Cause 4: fixture connecthealth-subscription-error-paths, step teardown#5 - DELETE /domain/{domainId}; inspect the 200 body for deletedAt and for status == DELETED (not DELETING). Re-replay connecthealth-subscription-lifecycle#teardown#9 too, which clusters with it.
7. Cause 5: fixture connecthealth-subscription-lifecycle, step setup#1 - POST /domain; inspect the 201 body for accountId.
8. All five steps must pass before this issue is closed.
