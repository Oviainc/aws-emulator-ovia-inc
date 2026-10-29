Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# The tag plane's 404 echoes a ResourceArn of up to 2048 characters into resourceName, modelled 1..256

_Filed: 2026-10-29T00:33:52Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three tag-plane operations take `ResourceArn` as a `TagrisArn`, modelled `@length(min: 20, max: 2048)`. When the ARN names no resource, each one answers `crate::state::no_such_resource(&input.resource_arn)`, which reports the ARN verbatim in the exception's own ARN member.

That member is `ResourceNotFoundException$resourceName`, which targets `InvoiceUnitArnString` — `@length(min: 1, max: 256)`. A perfectly legal `ResourceArn` of 300–2048 characters (the pattern lets the resource segment be any run of `[-a-zA-Z0-9/:_]`) therefore comes back inside a member the model caps at 256, and the walk validates only the input (vendor/smithy-server-runtime/src/operation.rs:412), so nothing clips it.

This is the crate's own rule, applied to the pattern but not to the length: the sibling helper `no_such_resource_unnamed` exists precisely because a value `InvoiceUnitArnString` does not admit "cannot be reported in it without answering a value the model says that member never holds", and `no_such_procurement_portal` leaves the same member absent for the same reason. The invoice-unit callers of `no_such_resource` are safe only because their `InvoiceUnitArn` input is itself capped at 256; the tag plane's input is capped eight times higher, and no comment or test notes the difference.

## Evidence

Smithy — what the tag plane accepts, `smithy_query(query="shape", shape="TagrisArn")`:
  {"type": "string", "length": {"min": 20, "max": 2048}, "pattern": "^arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+$"}
and what the 404 may carry, `smithy_query(query="shape", shape="ResourceNotFoundException")`:
  "resourceName": {"target": "InvoiceUnitArnString", "length": {"min": 1, "max": 256}, "pattern": "^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$"}

The helper, which echoes the ARN with no bound — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:1272-1278:
```
pub fn no_such_resource(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(RESOURCE_NOT_FOUND_MESSAGE.to_string()),
        resource_id: Some(arn.to_string()),
        message_key: MessageKey::ModelMemberName,
    }
}
```

The three tag-plane callers that hand it a `TagrisArn` — src/ops/list_tags_for_resource.rs:157-162:
```
        if !store.resource_exists(&input.resource_arn) {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                crate::state::no_such_resource(&input.resource_arn),
            )
            .into());
        }
```
the same five lines in src/ops/tag_resource.rs:158-163 and src/ops/untag_resource.rs:154-159.

The crate's OWN statement of the rule it breaks here — state.rs:1306-1312 (`no_such_resource_unnamed`):
```
/// The 404 a reader answers for a resource named by something that is NOT an ARN — an
/// invoice id, say: the same constant message, with the ARN member left unset.
///
/// That member is modelled `InvoiceUnitArnString`, whose `@pattern` admits only an ARN
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a
/// non-ARN identifier cannot be reported in it without answering a value the model
/// says that member never holds.
```
and the same reasoning again at state.rs:1023-1025 (`no_such_procurement_portal`):
```
/// `resourceName` is left absent: the model types it as an `InvoiceUnitArnString`
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`), so a portal
/// identifier — which is not an ARN — cannot be reported in it.
```

Only the input is validated, so the oversized member reaches the client — vendor/smithy-server-runtime/src/operation.rs:412:
```
        let violations = aws_smithy_schema_server::constraints::validate(&input, O::INPUT_SCHEMA);
```

Test coverage: the tag plane's 404s are pinned only with short ARNs — tests/all/invoice_unit_lifecycle.rs:654-663 uses `arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff` for `ListTagsForResource`, and tests/all/wire_envelope_and_error_detail.rs:464-469 exercises `TagrisArn` only with a too-SHORT/wrong-service value (`"arn:aws:s3:::b"`). `grep -n '2048|repeat(' tests/all/tag_write_quota_counts_distinct_keys.rs` matches nothing, so no test asserts or excuses a long ARN being echoed back.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.ListTagsForResource` and body `{"ResourceArn":"arn:aws:invoicing::000000000000:" + 300 copies of 'a'}` — a value inside `TagrisArn`'s `@length(20..2048)` and matching its pattern, naming no resource this service holds.
3. Observe HTTP 404 `ResourceNotFoundException` whose `resourceName` is the full 332-character ARN.
4. Compare with the model: `resourceName` targets `InvoiceUnitArnString`, `@length(min: 1, max: 256)`, so the body reports a value the model says that member never holds — the same objection the crate itself raises for a non-ARN identifier in `state.rs:1306-1312`.
5. Repeat with `X-Amz-Target: Invoicing.TagResource` (`{"ResourceArn":"<same>","ResourceTags":[]}`) and `Invoicing.UntagResource` (`{"ResourceArn":"<same>","ResourceTagKeys":[]}`): both answer the same oversized `resourceName`, since all three call the one helper.
