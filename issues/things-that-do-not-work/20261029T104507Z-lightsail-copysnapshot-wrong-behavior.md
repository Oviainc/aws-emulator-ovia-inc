Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# The crate answers "what operationType does a call the modelled enum has no member for log?" two contradictory ways — minted for ExportSnapshot/DeleteKeyPair/CreateCloudFormationStack, omitted for CopySnapshot/TagResource/UntagResource/DeleteAutoSnapshot

_Filed: 2026-10-29T10:45:07Z_
_Operation: CopySnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Seven mutating handlers in this crate write an operation-log entry for a call that the modelled `OperationType` enum declares no member for (verified from the model: the enum carries no `CopySnapshot`, `TagResource`, `UntagResource`, `DeleteAutoSnapshot`, `ExportSnapshot`, `CreateCloudFormationStack` or `DeleteKeyPair`). The crate states two mutually exclusive rules for that one situation and applies each to a different subset of those seven calls:

* `record_untyped_operation` / `start_untyped_operation` state the rule as "leave the optional member absent rather than reporting a value the model does not define", and `CopySnapshot`, `TagResource`, `UntagResource` and `DeleteAutoSnapshot` follow it — their entries carry no `operationType` at all.
* `OPERATION_TYPE_EXPORT_SNAPSHOT` states the opposite rule — "A Smithy enum is open on the wire (a client has to tolerate a value it does not know), so the honest entry names the operation that ran, exactly as every other value here does" — and `ExportSnapshot`, `CreateCloudFormationStack` and `DeleteKeyPair` follow it, minting the unmodelled strings `"ExportSnapshot"`, `"CreateCloudFormationStack"` and `"DeleteKeyPair"`.

Both passages are about the SAME question and reach opposite conclusions, so one of them is wrong and `GetOperations` / `GetOperationsForResource` report the account's log inconsistently: an export, a stack create and a key-pair delete are each named by an operationType outside the modelled enum, while a snapshot copy, a tag change and an automatic-snapshot delete are reported with no operationType at all. The second rule's own wording ("exactly as every other value here does") is false of the crate as it stands, and the crate's own shared test helper for the log asserts the first rule is universal ('every operation-log entry carries an operationType'), which the four omitting handlers break.

The defect is the contradiction, not a particular value: whichever rule is the right one, it should be stated once and applied to all seven calls.

## Evidence

Rule A — omit the member. services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:14990-15001:
```
    /// Build and record a TERMINAL operation against `resource_name` that
    /// reports no `operationType`.
    ///
    /// The modelled `OperationType` enum declares a member for most calls that
    /// write this log and none for a change to a resource's tags or for the
    /// deletion of one automatic snapshot, so an entry for either leaves
    /// `Operation$operationType` — an optional member — absent rather than
    /// reporting a value the model does not define.
```
and its non-terminal twin, src/state.rs:15011-15020:
```
    /// The counterpart of [`Self::record_untyped_operation`] for a call the
    /// modelled `OperationType` enum declares no value for whose work is NOT
    /// finished when the call returns — copying a snapshot, where the copy is
    /// still being taken as the call answers, so the entry stays `Started` until
    /// the clock settles it.
    pub fn start_untyped_operation(
```

Rule B — mint an unmodelled value. src/state.rs:761-772:
```
/// The `operationType` the log entry of an `ExportSnapshot` call carries.
///
/// `Operation$operationType` targets the modelled `OperationType` enum, whose
/// members this module otherwise keeps to (see
/// [`OPERATION_TYPE_UPDATE_DOMAIN_ENTRY`], where a modelled value states exactly
/// the same fact). That enum predates the Amazon EC2 export family — it carries
/// no member for `ExportSnapshot`, and none for `CreateCloudFormationStack`
/// either — and no member it does carry states what an export did: logging one
/// as a snapshot CREATE would misreport the call. A Smithy enum is open on the
/// wire (a client has to tolerate a value it does not know), so the honest entry
/// names the operation that ran, exactly as every other value here does.
pub const OPERATION_TYPE_EXPORT_SNAPSHOT: &str = "ExportSnapshot";
```
and the same reasoning re-used for the key pair, src/state.rs:773-782 (`pub const OPERATION_TYPE_DELETE_KEY_PAIR: &str = "DeleteKeyPair";`) and for the stack, src/state.rs:6193-6197 (`pub const OPERATION_TYPE_CREATE_CLOUD_FORMATION_STACK: &str = "CreateCloudFormationStack";`).

The two sides in the handlers. src/ops/copy_snapshot.rs:311-324 (omits):
```
            // `operations` is a list because other Lightsail calls act on
            // several resources at once; one `CopySnapshot` creates exactly one
            // snapshot, so it answers exactly one entry — against the copy, the
            // resource the call created — left non-terminal until the copy the
            // clock owes it is due. It reports no `operationType`: the modelled
            // `OperationType` enum declares no value for this call, and an
            // absent optional member is the model's answer to that.
            let operation = store.start_untyped_operation(
```
src/ops/export_snapshot.rs:243-250 (mints):
```
            let operation = store.start_operation(
                now,
                crate::state::OPERATION_TYPE_EXPORT_SNAPSHOT,
                resource_type,
                &source_snapshot_name,
```
(the same split: src/ops/tag_resource.rs:272, src/ops/untag_resource.rs:238 and src/ops/delete_auto_snapshot.rs:202 all call `record_untyped_operation`; src/ops/delete_key_pair.rs:227 and src/ops/create_cloud_formation_stack.rs:340 pass a minted constant.)

The model, for both sides of the split — smithy_query(query="shape", shape="com.amazonaws.lightsail#OperationType") lists 84 values, among them `"UpdateDomainEntry"`, `"DeleteKnownHostKeys"` and `"CreateBucketAccessKey"`, and NONE of `CopySnapshot`, `TagResource`, `UntagResource`, `DeleteAutoSnapshot`, `ExportSnapshot`, `CreateCloudFormationStack`, `DeleteKeyPair`. So all seven calls are in the identical position.

The crate's own log reader states rule A as an invariant — tests/all/operations_for_resource.rs:194-204, the helper shared with tests/all/operations_log.rs, tests/all/vpc_peering_operation_log.rs and tests/all/export_snapshot_source_kinds.rs:
```
/// The `operationType` of each entry of `entries`, in order. Shared with the
pub(crate) fn operation_types(entries: &[Value]) -> Vec<String> {
...
                    panic!("every operation-log entry carries an operationType, got {entry:?}")
```
which the four omitting handlers contradict (that helper panics on any entry they wrote).

Test coverage, both sides: tests/all/snapshot_copy.rs:586-605 pins the absent member for CopySnapshot ("the model declares no operationType for this call"), tests/all/resource_tags.rs:264-302 and tests/all/resource_tagging.rs:433-439 pin it for the tag calls, tests/all/auto_snapshot_delete.rs:124-160 for DeleteAutoSnapshot — while tests/all/export_snapshot_source_kinds.rs:360-364 pins the unmodelled `"ExportSnapshot"` value, tests/all/cloud_formation_stack.rs:491 the unmodelled stack value and tests/all/key_pair_delete.rs:260 the unmodelled `"DeleteKeyPair"`. No test compares the two groups, which is why the contradiction has stood.

## Steps to Reproduce

1. Create an instance and let it settle, then CreateInstanceSnapshot with instanceSnapshotName "snap-a" and wait for it to become available.
2. Call ExportSnapshot with sourceSnapshotName "snap-a", then GetOperationsForResource with resourceName "snap-a": the entry reports operationType "ExportSnapshot" — a value smithy_query(query="shape", shape="OperationType") does not list.
3. Call CopySnapshot with sourceSnapshotName "snap-a", targetSnapshotName "snap-b" and sourceRegion of the snapshot, then GetOperationsForResource with resourceName "snap-b": the entry reports NO operationType at all, although its situation is identical (the enum lists no CopySnapshot either).
4. Call TagResource for "snap-b" and DeleteAutoSnapshot for a resource with an automatic snapshot, then DeleteKeyPair and CreateCloudFormationStack: the first two entries carry no operationType, the last two carry the unmodelled strings "DeleteKeyPair" and "CreateCloudFormationStack".
5. Read src/state.rs:14990-15001 ("leaves `Operation$operationType` — an optional member — absent rather than reporting a value the model does not define") next to src/state.rs:761-772 ("A Smithy enum is open on the wire ... so the honest entry names the operation that ran, exactly as every other value here does"): the two state opposite rules for the same question, and the second's "exactly as every other value here does" is untrue of the four calls that follow the first.
