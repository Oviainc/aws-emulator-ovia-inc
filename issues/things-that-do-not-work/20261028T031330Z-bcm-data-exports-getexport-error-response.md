Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# ResourceNotFoundException.ResourceId echoes the whole ARN, past its modelled @length(0, 1024)

_Filed: 2026-10-28T03:13:30Z_
_Operation: GetExport | Class: error-response_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every not-found refusal of this service is built by the one helper `crate::state::resource_not_found`, which copies the request's ARN verbatim into `ResourceNotFoundException$ResourceId`. The model bounds that member at 1024 characters, while the ARN it is filled from is a modelled `Arn` of up to 2048 characters that the boundary validator hands straight to the handler. So a well-formed ARN of this account longer than 1024 characters that names no export produces a wire response whose `ResourceId` violates the member's own modelled `@length(0, 1024)` (and its `Message`/`ResourceType` siblings are inside it only because they are constants).

The defect is in the shared helper, so it reaches every ARN-addressed not-found path of this service: `GetExport`, `UpdateExport`, `DeleteExport` (via `export_not_found_error`), `GetExecution` (via `execution_export_not_found`) and `TagResource` / `UntagResource` / `ListTagsForResource` (via `tagged_resource_not_found_error`).

This is the one echoed caller value the crate does NOT hold inside its member's modelled bound: `field_validation_failed` clips the `ValidationException` message and both `ValidationExceptionField` members to `MESSAGE_LENGTH_LIMIT`, and `state::authorize` deliberately clips the echoed ARN into `AccessDeniedException$Message` for exactly this reason (an `Arn` "admits twice its characters"). Only the not-found path skips it. No crate comment or test justifies exceeding the bound: the comment on the line only argues against SHORTENING for prose, and the test it cites tops out at an ARN of about 1021 characters — inside the bound — so a fix that keeps the ARN whole whenever it fits leaves every existing test unchanged.

## Evidence

Smithy (smithy_query query="shape" shape="ResourceNotFoundException"): `"ResourceId": {"target": "GenericString", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^[\\S\\s]*$", "doc": "<p>The identifier of the resource that was not found.</p>"}`. The value it is filled from is unbounded by that: OPERATIONS.md:83 `- **Input:** ExportArn (string, len 20..2048, pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$, required)`.

Handler side — services/.../src/state.rs:683-691:
```
pub fn resource_not_found(arn: &str, message: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.to_string(),
        // `ResourceId` reports the identifier WHOLE rather than shortened: it is the
        // identity of what was not found, not prose about it, and a client reads it to
        // know which of its own requests failed (`tests/all/not_found_message.rs`).
        resource_id: arn.to_string(),
```
Reached by services/.../src/ops/get_export.rs:161-166 `store.exports.get(&input.export_arn).ok_or_else(|| crate::state::export_not_found_error(&input.export_arn, GetExportErrorModeled::ResourceNotFoundException))` (and the same helper from delete_export.rs:165, update_export.rs:260, get_execution.rs:191, tag_resource.rs:196, untag_resource.rs:201, list_tags_for_resource.rs:193).

Same crate, opposite rule for the sibling member — services/.../src/state/authorization.rs:124-128: "`AccessDeniedException$Message` is the same `GenericString` every refusal of this service is worded into, while the modelled `Arn` the sentence echoes admits twice its characters — so the ECHOED RESOURCE takes the room the clause around it leaves", implemented at authorization.rs:139-145 with `crate::state::clip_to_budget(resource, room)`; and services/.../src/state.rs:1218-1222 `message: clip_to_budget(&message, MESSAGE_LENGTH_LIMIT)` / `name: clip_to_budget(member, MESSAGE_LENGTH_LIMIT)`.

The outsized ARN is proven to reach a handler by this crate's own test — services/.../tests/all/error_member_bounds.rs:32-52: `/// The @length ceiling of an ARN this service accepts (smithy_query shape Arn: @length(min: 20, max: 2048)). const MAX_ARN_CHARS: usize = 2048;` with `padded_arn(chars)` building a pattern-satisfying ARN of that length that the boundary validator passes through (row `MAX_ARN_CHARS`).

Test coverage of the not-found body: services/.../tests/all/not_found_message.rs:244-248 asserts `body["ResourceId"] == arn`, over rows whose longest ARN is `let long_tail = "segment_one/segment-two:segment3/".repeat(30);` (not_found_message.rs:71) — about 1021 characters, i.e. still inside `@length(0, 1024)`, so nothing pins an over-bound ResourceId.

## Steps to Reproduce

1. Build a well-formed ARN of this account naming no export, 2048 characters long, the way tests/all/error_member_bounds.rs does: `engine::emulator_arn("bcm-data-exports", "us-east-1", "export/absent")` padded with 'a' inside the resource segment until it is 2048 characters (it satisfies the modelled `Arn` pattern, so the boundary validator hands it to the handler).
2. POST / with `X-Amz-Target: AWSBillingAndCostManagementDataExports.GetExport` and body `{"ExportArn": "<that 2048-character ARN>"}`.
3. Observe the `ResourceNotFoundException` body: `ResourceId` is the full 2048-character ARN, exceeding the member's modelled `@length(min: 0, max: 1024)`.
4. Repeat with `DeleteExport`, `UpdateExport`, `GetExecution`, `TagResource`, `UntagResource` and `ListTagsForResource` for the same ARN — each answers the same over-bound `ResourceId`, since all of them build the refusal through `crate::state::resource_not_found`.
5. Contrast with the same ARN sent by a principal with no grant: `UntagResource` answers `AccessDeniedException` whose `Message` IS clipped to 1024 characters (tests/all/error_member_bounds.rs), showing the rule the not-found path skips.
