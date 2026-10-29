Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# A change against an entity id a running change set has RESERVED answers 404, not the documented ResourceInUseException

_Filed: 2026-10-28T23:37:43Z_
_Operation: StartChangeSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartChangeSet mints an entity id for every change of an `Intent=APPLY` change set that names no `Entity.Identifier`, stores it on the change record, and reports it in `ChangeSetSummaryListItem$EntityIdList` while the change set is still PREPARING/APPLYING. The crate treats that reserved id as LOCKED: `mint_entity_id` refuses to re-mint it and `locking_change_set` returns the holding change set for it.

But the handler resolves `Entity.Identifier` against `store.entities` BEFORE it consults the lock. For an id that is reserved-but-not-yet-written (the creating change set has not reached SUCCEEDED, so `apply_changes` has not run), `resolve_identifier` returns `None` and the request is refused `ResourceNotFoundException` (404) — even though `store.locking_change_set(entity_id)` would name the change set holding it. The lock branch that answers the modelled `ResourceInUseException` is only reachable once the entity exists, so the documented lock refusal is unreachable for exactly the window in which the entity is in flight.

## Evidence

Modelled operation documentation for StartChangeSet (quoted verbatim in the handler header, services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:49-53):

    //! ChangeSet is running, all the entities targeted by the different changes are locked
    //! until the change set has completed (either succeeded, cancelled, or failed). If you try
    //! to start a change set containing a change against an entity that is already locked, you
    //! will receive a ResourceInUseException error.

The handler refuses on absence first — start_change_set.rs:567-581:

    let held = match target_change.entity.identifier.as_deref() {
        Some(identifier) => {
            let entity_id = crate::state::entity_id_of(identifier);
            let Some(entity) = store.resolve_identifier(identifier) else {
                return Err(
                    StartChangeSetErrorModeled::ResourceNotFoundException(
                        crate::state::entity_not_found(entity_id),
                    )
                    .into(),
                );
            };

and only afterwards consults the lock — start_change_set.rs:619-631:

    // "when a ChangeSet is running, all the entities targeted by
    // the different changes are locked until the change set has
    // completed … If you try to start a change set containing a
    // change against an entity that is already locked, you will
    // receive a ResourceInUseException error."
    if let Some(holder) = store.locking_change_set(entity_id) {
        return Err(StartChangeSetErrorModeled::ResourceInUseException(

The crate itself says the reserved id is observable and locked — start_change_set.rs:537-541:

    // A change naming an entity acts on that entity; a change naming
    // none creates one, and the id it will carry is reserved now so
    // `EntityIdList` can report it while the change set runs.

and src/state.rs:1466-1478:

    /// Mint an unused entity id matching `ResourceId`'s `^[\w\-]+$`.
    ///
    /// The loop also skips an id a running change set has already reserved, so
    /// two change sets in flight never name the same new entity.
    …  if !self.entities.contains_key(&id) && self.locking_change_set(&id).is_none() {

state.rs:1505-1522 (`locking_change_set`) documents itself as THE lock: "The id of the still-running change set that has `entity_id` locked", and state.rs:769-779 (`entity_ids`) feeds those reserved ids into `ChangeSetSummaryListItem$EntityIdList`.

Test coverage: the lock refusal is pinned only for entities that already exist — tests/all/change_pair_uniqueness.rs:502-565 (`an_entity_a_running_change_set_holds_is_refused_as_in_use`, built on `created_entity`) and tests/all/change_set_lifecycle.rs:1602-1638 (`a_running_change_set_locks_its_entities`, also on a created entity). No test covers a change against an id reported by `EntityIdList` while the creating change set is still running.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace, Intent=APPLY and ChangeSet=[{ChangeType:"CreateProduct", Entity:{Type:"AmiProduct"}, DetailsDocument:{"ProductTitle":"p"}}] — note the returned ChangeSetId.
2. Immediately (inside CHANGE_SET_PREPARING + CHANGE_SET_APPLYING, i.e. ~500 ms) POST /ListChangeSets with Catalog=AWSMarketplace and read EntityIdList[0] of that change set — it names the reserved entity id ent-XXXXXXXXXXXX while Status is PREPARING.
3. Still inside that window, POST /StartChangeSet with ChangeSet=[{ChangeType:"UpdateInformation", Entity:{Type:"AmiProduct", Identifier:"ent-XXXXXXXXXXXX"}, DetailsDocument:{"ProductTitle":"q"}}].
4. Observed: 404 ResourceNotFoundException "Requested entity 'ent-XXXXXXXXXXXX' does not exist." Expected per the quoted documentation (and per the crate's own locking_change_set, which names the holder for that id): 423 ResourceInUseException naming the locked entity and the change set holding it.
