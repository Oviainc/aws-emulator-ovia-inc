Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c8784923130dd4420e009c5117a8bbfb746443f0

# The batch per-entry outcome machinery and the batch id-count bound are written out three times, once per Batch operation

_Filed: 2026-10-29T10:44:40Z_
_Operation: BatchDelete | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c8784923130dd4420e009c5117a8bbfb746443f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchDelete`, `BatchStart` and `BatchStop` each carry their own verbatim copy of the same two pieces of logic, so the three copies can drift from each other:

1. The per-entry outcome machinery: the `Results` struct with its `push` match, the `Outcome` enum, the `failure_code` module's `NOT_FOUND`/`CONFLICT` constants, and the `not_found`, `refused` and `accepted` constructors. These are byte-identical across the three files (`BatchDelete` adds one extra constant, `failure_code::BAD_REQUEST`, and each `accepted` differs only in the operation's own error type).

2. The batch id-count bound: "count every id list, compare against this operation's cap, answer the modelled `BadRequestException` with the message `\"<Op> names {named} resources, more than the {cap} one call may name\"`" — the same five-statement block three times over three constants (`MAX_BATCH_DELETE_IDS`, `MAX_BATCH_START_IDS`, `MAX_BATCH_STOP_IDS`).

The drift this invites is not hypothetical for this family: an earlier issue in this crate (20261028T160823Z-medialive-startmultiplex-error-response) already records that `BatchStart`'s own `failure_code` module states the not-found-before-conflict rule while its multiplex arm does not follow it, i.e. the copies of the classification rule and its use are already out of step. Folding the machinery into one place (a private shared module in `crate::ops`, generic over the operation's error type, taking the cap and the operation name) leaves every response byte-identical: the codes, the message wording and the per-entry shapes are all already the same text.

## Evidence

Copy 1 — services/service-com-amazonaws-medialive-medialive/src/ops/batch_delete.rs:217-302:
```
/// This service's state, named once so the per-resource helpers below read.
type State = crate::state::ComAmazonawsMedialiveMedialiveState;

/// The two output lists, filled in the order the request listed the resources.
#[derive(Debug, Default)]
struct Results {
    failed: Vec<BatchFailedResultModel>,
    successful: Vec<BatchSuccessfulResultModel>,
}

impl Results {
    fn push(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Successful(result) => self.successful.push(result),
            Outcome::Failed(result) => self.failed.push(result),
        }
    }
}
...
fn refused(id: &str, arn: Option<String>, code: &str, message: String) -> Outcome {
    Outcome::Failed(BatchFailedResultModel {
        arn,
        code: Some(code.to_string()),
        id: Some(id.to_string()),
        message: Some(message),
    })
}
```

Copy 2 — src/ops/batch_start.rs:212-274, the same declarations in the same order:
```
/// This service's state, named once so the per-resource helpers below read.
type State = crate::state::ComAmazonawsMedialiveMedialiveState;

/// The two output lists, filled in the order the request listed the resources.
#[derive(Debug, Default)]
struct Results {
    failed: Vec<BatchFailedResultModel>,
    successful: Vec<BatchSuccessfulResultModel>,
}
...
fn refused(id: &str, arn: Option<String>, code: &str, message: String) -> Outcome {
    Outcome::Failed(BatchFailedResultModel {
        arn,
        code: Some(code.to_string()),
        id: Some(id.to_string()),
        message: Some(message),
    })
}
```

Copy 3 — src/ops/batch_stop.rs:212-292, the same again:
```
/// This service's state, named once so the per-resource helpers below read.
type State = crate::state::ComAmazonawsMedialiveMedialiveState;
...
fn not_found(resource_type: &str, id: &str) -> Outcome {
    Outcome::Failed(BatchFailedResultModel {
        arn: None,
        code: Some(failure_code::NOT_FOUND.to_string()),
        id: Some(id.to_string()),
        message: Some(crate::state::not_found_message(resource_type, id)),
    })
}
```

The bound, three times. src/ops/batch_delete.rs:169-185:
```
        let named = input.channel_ids.as_ref().map_or(0, Vec::len)
            + input.input_ids.as_ref().map_or(0, Vec::len)
            + input.input_security_group_ids.as_ref().map_or(0, Vec::len)
            + input.multiplex_ids.as_ref().map_or(0, Vec::len);
        if named > crate::state::MAX_BATCH_DELETE_IDS {
            return Err(BatchDeleteErrorModeled::BadRequestException(BadRequestException {
                message: Some(format!(
                    "BatchDelete names {named} resources, more than the {} one call may name",
                    crate::state::MAX_BATCH_DELETE_IDS
                )),
```
src/ops/batch_start.rs:172-181:
```
        let named = input.channel_ids.as_ref().map_or(0, Vec::len)
            + input.multiplex_ids.as_ref().map_or(0, Vec::len);
        if named > crate::state::MAX_BATCH_START_IDS {
            return Err(BatchStartErrorModeled::BadRequestException(BadRequestException {
                message: Some(format!(
                    "BatchStart names {named} resources, more than the {} one call may name",
```
src/ops/batch_stop.rs:172-181:
```
        let named = input.channel_ids.as_ref().map_or(0, Vec::len)
            + input.multiplex_ids.as_ref().map_or(0, Vec::len);
        if named > crate::state::MAX_BATCH_STOP_IDS {
            return Err(BatchStopErrorModeled::BadRequestException(BadRequestException {
                message: Some(format!(
                    "BatchStop names {named} resources, more than the {} one call may name",
```

Test coverage that must stay unchanged: tests/all/batch_delete.rs:194 (`every_deletable_resource_is_accepted_and_reported_in_the_state_it_reached`), :353 (`every_resource_a_delete_cannot_touch_is_reported_as_its_own_failure`), :537 (`one_call_reports_the_accepted_and_the_refused_side_by_side`), :655 (`a_batch_above_the_bound_is_refused_whole`); the same four shapes in tests/all/batch_start.rs and tests/all/batch_stop.rs:170/276/445/548/665; and the cross-operation classification test tests/all/post_delete_mutation_classification.rs:238-293 (`a_batch_entry_for_a_deleted_channel_reports_not_found`), which asserts `"code": "NotFoundException"` for all three batch forms.

## Steps to Reproduce

1. Open src/ops/batch_delete.rs, src/ops/batch_start.rs and src/ops/batch_stop.rs side by side.
2. Compare batch_delete.rs:217-302 with batch_start.rs:212-274 and batch_stop.rs:212-292: `type State`, `struct Results` + `impl Results::push`, `enum Outcome`, `mod failure_code` (NOT_FOUND/CONFLICT), `fn not_found`, `fn refused` and `fn accepted` are the same code three times, differing only in the operation's error type and in BatchDelete's extra BAD_REQUEST constant.
3. Compare batch_delete.rs:169-185 with batch_start.rs:172-181 and batch_stop.rs:172-181: the id-count bound and its message are the same block three times over three constants.
4. Fold both into one private shared helper in `crate::ops` parameterised by the operation name, its cap and its error type, and re-run tests/all/batch_delete.rs, tests/all/batch_start.rs, tests/all/batch_stop.rs and tests/all/post_delete_mutation_classification.rs: every response and every assertion is unchanged.
