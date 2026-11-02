Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.concurrent-associate-assets-race
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise.concurrent-associate-assets-race|run#13|AssertionError|assert all(msg == "Child asset already has a parent" for _code, msg in losers), outcomes
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# A settled duplicate AssociateAssets is refused with the emulator's own 'already associated' sentence instead of 'Child asset already has a parent'

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAssets

## Description
Cause 1 of 1: once an association has settled, a repeat AssociateAssets for the same (assetId, hierarchyId, childAssetId) triple is refused with the right modeled error class but a message the emulator authored itself, carrying three interpolated ids AWS's message does not have.
AWS: Smithy: AssociateAssets input assetId (httpLabel), hierarchyId and childAssetId (body, @required); errors InvalidRequestException (httpError 400, member message), ConflictingOperationException (409), ResourceNotFoundException (404). Re-associating a triple whose association has ALREADY settled - so no operation is in flight - is refused InvalidRequestException / HTTP 400 with the message 'Child asset already has a parent'. That message is entirely FIXED text: it has no variable parts, in particular no asset or hierarchy ids, and no trailing period. Under concurrency the only two refusals AWS answers a losing racer are this one and the hierarchy-scoped 409 'A hierarchy with ID <hierarchyId> already exists'; exactly one racer wins and the child ends up associated exactly once.
Emulator: each losing racer (after the harness has retried past any 409) is answered InvalidRequestException with 'The asset <childAssetId> is already associated with the asset <parentAssetId> through the hierarchy <hierarchyId>.' - a sentence this service does not produce, with three interpolated ids and a trailing period AWS's message does not have.
Why: clients and tests classify this refusal on its text - it is the one refusal that distinguishes a terminal 'the child already has a parent' from a retryable conflict; matching AWS's documented wording fails, and log-based diagnosis reads a different condition than the one AWS reports.
Fix: the AssociateAssets handler's duplicate/already-parented refusal must emit AWS's fixed sentence verbatim rather than the crate's id-interpolated template. Related open issue 20261029T194801Z-iotsitewise-asset-hierarchy-refusals-answer-the-wrong-error-class.md owns the sibling checks associate-assets-duplicate-rejected and associate-assets-concurrent-duplicate-race (its causes 1-2 concern the error CLASS and the 409 wording); it does not carry this check's replay key, and what this check observes is specifically the 400's MESSAGE on the already-settled duplicate path, so it is filed separately and should be closed together with that issue.
Traces: iotsitewise.concurrent-associate-assets-race.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 54/87 passed, all 33 failures in iotsitewise.

iotsitewise.concurrent-associate-assets-race - six concurrent identical AssociateAssets calls for one (parent asset, hierarchy, child asset) triple, each retrying while it sees ConflictingOperationException. The check asserts exactly one SUCCESS, every loser code == 'InvalidRequestException', and every loser message == 'Child asset already has a parent'. Result: exactly one ('SUCCESS', None) and five ('InvalidRequestException', 'The asset <childAssetId> is already associated with the asset <parentAssetId> through the hierarchy <hierarchyId>.'), so the class is right and the message is the emulator's. The AssertionError is raised on the message equality and prints the full outcome list.

The ids in the quoted message are that run's values filling the emulator's own template; AWS's message has no variable parts at all - 'Child asset already has a parent' is fixed text end to end.

Model grounding: AssociateAssets declares InvalidRequestException (400), ConflictingOperationException (409) and ResourceNotFoundException (404), each with a message member.

## Steps to Reproduce
1. Create a parent asset model with a hierarchy over a child asset model; create one parent asset and one child asset; wait for both ACTIVE.
2. Fire several identical POST /assets/{assetId}/associate calls concurrently for the same (assetId, hierarchyId, childAssetId) triple, retrying each on ConflictingOperationException.
3. Observe exactly one success, and every other call refused 400 InvalidRequestException with 'The asset <childAssetId> is already associated with the asset <parentAssetId> through the hierarchy <hierarchyId>.' instead of the fixed 'Child asset already has a parent'.
4. Re-replay iotsitewise.concurrent-associate-assets-race to confirm.
