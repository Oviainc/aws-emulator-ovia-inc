Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# Two changes of the same change type against one entity answer 422 ValidationException instead of 423 ResourceInUseException

_Filed: 2026-10-24T01:04:54Z_
_Operation: StartChangeSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a single `ChangeSet` contains two changes that run the same `ChangeType` against the same entity, the emulator refuses the request with 422 `ValidationException`. The operation's own documentation presents exactly this case as the `ResourceInUseException` example: "when a ChangeSet is running, all the entities targeted by the different changes are locked … If you try to start a change set containing a change against an entity that is already locked, you will receive a ResourceInUseException error. For example, you can't start the ChangeSet described in the example later in this topic because it contains two changes to run the same change type (AddRevisions) against the same entity (entity-id@1)." Expected: 423 `ResourceInUseException`. Actual: 422 `ValidationException`.

## Evidence

Spec: `StartChangeSet` documentation (model `com.amazonaws.marketplacecatalog#StartChangeSet`) ties the same-change-type-twice-against-one-entity example directly to `ResourceInUseException`; the operation models `ResourceInUseException` (HTTP 423).
Handler: `src/ops/start_change_set.rs` lines ~252-268 return `ValidationException(crate::state::duplicate_change_pair(..))` for the duplicate (entity, change type) pair, while the cross-change-set lock at lines ~439-453 correctly returns `ResourceInUseException`.
Tests: `tests/all/change_set_lifecycle.rs` lines 1621-1657 assert the current (422, "ValidationException") answer, so the divergence is pinned, not uncovered.

## Steps to Reproduce

1. StartChangeSet to create an AmiProduct entity; wait for SUCCEEDED and note the EntityId
2. POST /StartChangeSet with Catalog=AWSMarketplace and a ChangeSet containing two changes, both ChangeType=AddRevisions, both Entity={Type:AmiProduct, Identifier:<EntityId>}
3. Observe 422 ValidationException; AWS answers 423 ResourceInUseException for this request
