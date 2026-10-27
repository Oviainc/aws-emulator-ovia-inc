Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f2ead5306d42c2c14ef865f57d355d9f40696e5a

# Tags are writable on a DELETED multiplex, the one tombstone the tag-write path forgets to filter

_Filed: 2026-10-27T18:24:13Z_
_Operation: CreateTags | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f2ead5306d42c2c14ef865f57d355d9f40696e5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateTags` and `DeleteTags` resolve an ARN to a writable tag map through the private `ComAmazonawsMedialiveMedialiveState::resource_tags_mut`. That function's own documentation states the crate's rule: a resource whose delete has been accepted is NOT handed out, and names exactly ONE exception (the input security group, whose delete is a soft delete). Every tombstone-keeping arm of the match therefore filters its dead records — Channel, Input, Network, Node, Cluster, ChannelPlacementGroup and Reservation all do — except the MULTIPLEX arm, which hands out the tag map of any multiplex, including one that is `DELETING` or has reached the terminal `DELETED`.

A multiplex is exactly such a tombstone: `MultiplexRecord::begin_delete` moves it to `DELETING` and owes the terminal `DELETED` state "it then keeps" because "the record stays in the account", and the module already owns the predicate for it (`MultiplexRecord::delete_accepted`), used by `accepts_programs()` and `is_updatable()` so that `CreateMultiplexProgram`/`UpdateMultiplexProgram`/`DeleteMultiplexProgram`/`UpdateMultiplex` all refuse a multiplex whose delete was accepted.

So `POST /prod/tags/<deleted-multiplex-arn>` answers 204 and the tag is written onto the dead record, where the documented rule (and every sibling resource) answers 404 `NotFoundException` and writes nothing. `ListTagsForResource` and `DescribeMultiplex` then report a tag set that was mutated after the resource was deleted. The same applies to `DeleteTags`, which reaches the same helper through `remove_resource_tags`.

## Evidence

The rule, stated by the helper the tag writers call — `services/service-com-amazonaws-medialive-medialive/src/state.rs:4346-4356`:
```
/// A resource whose delete has been accepted is NOT handed out: tagging and
/// untagging change a resource's configuration, so they resolve the ARN
/// through the same live lookup every other mutation uses
/// ([`Self::live_input`]) and the tag writers answer a dead ARN with their
/// modelled not-found error. ...
/// The INPUT SECURITY GROUP is handed out even once it is deleted, because
/// its delete is a SOFT delete
```
The sibling arm that obeys it — `src/state.rs:4363-4367`:
```
TaggedResource::Channel(id) => self
    .channels
    .get_mut(&id)
    .filter(|record| !record.is_delete_accepted())
    .map(|record| &mut record.tags),
```
The arm that breaks it — `src/state.rs:4391-4394`:
```
TaggedResource::Multiplex(id) => self
    .multiplexes
    .get_mut(&id)
    .map(|record| &mut record.tags),
```
That a deleted multiplex is a readable tombstone, and that the crate already has the predicate plus the "nothing may write to it again" rule — `src/state/multiplexes.rs:922-929`:
```
/// Move this multiplex into `DELETING`, owing the terminal `DELETED` state
/// it then keeps once [`TRANSIENT_STATE_SPAN`] has passed: the record stays
/// in the account, so a reader goes on reporting the multiplex in that state
```
and `src/state/multiplexes.rs:834-840`:
```
/// Whether a delete has already been accepted for this multiplex: it is
/// `DELETING`, or it has reached the terminal `DELETED`. The ONE predicate
/// behind both rules below, so the two cannot drift apart — the id names a
/// record a reader goes on reporting, and nothing may write to it again.
fn delete_accepted(&self) -> bool {
```
The write is observable on the read path — `src/state.rs:4307`: `TaggedResource::Multiplex(id) => self.multiplexes.get(&id).map(|record| &record.tags),`.

Test coverage: `tests/all/deleted_resource_mutation.rs:10-30` states the rule under test ("an operation that CHANGES that resource's configuration resolves the id to a live resource or answers its modelled `NotFoundException` (HTTP 404) and writes nothing") and names the single exception ("The INPUT SECURITY GROUP does NOT follow it"); its table at line 118 is `[Row; 4]` — input, channel, SdiSource, network — with `CREATE_TAGS`/`DELETE_TAGS` as mutations. No multiplex row exists, so nothing pins the current behaviour. The handler entry point is `src/ops/create_tags.rs:197-198` (`state.insert_resource_tags(&input.resource_arn, requested)`) and `src/ops/delete_tags.rs:185-186`.

## Steps to Reproduce

1. POST /prod/multiplexes with a valid multiplexSettings body and a requestId; note the returned multiplex id and arn
2. Poll GET /prod/multiplexes/<id> until state is IDLE
3. DELETE /prod/multiplexes/<id> — accepted; poll until state is DELETED (the record is kept, as designed)
4. POST /prod/tags/<url-encoded multiplex arn> with {"tags":{"phase":"after"}} — expected 404 NotFoundException with nothing written (the rule stated at state.rs:4346-4356 and asserted for channel/input/SdiSource/network in tests/all/deleted_resource_mutation.rs); the emulator answers 204
5. GET /prod/tags/<url-encoded multiplex arn> — the tag written after the delete is reported
6. DELETE /prod/tags/<arn>?tagKeys=phase — likewise answers 204 and removes the tag from the dead record
