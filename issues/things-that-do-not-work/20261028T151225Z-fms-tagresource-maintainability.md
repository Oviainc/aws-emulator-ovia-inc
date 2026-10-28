Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8d66e3013bd632dac86bbefa8efd5ff8af0fdc07

# The "is this string an ARN" rule is implemented twice in state.rs (arn_format_error and parse_arn)

_Filed: 2026-10-28T15:12:25Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8d66e3013bd632dac86bbefa8efd5ff8af0fdc07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` states the SAME ARN-form rule — the literal `arn:` prefix, six colon-separated fields, and a non-empty partition, service and resource, with the region and account fields allowed to be empty — in two separate functions, each with its own hand-written implementation:

* `arn_format_error` (the check `TagResource`, `UntagResource` and `ListTagsForResource` run on `ResourceArn` before any lookup), which answers a message;
* `parse_arn` (the parse `resource_set_item_failure_reason`, `decode_uri_page_token`, `in_region`, `emulator_partition` and `ListResourceSetResources`' `resource_account_id` run), which answers `Option<ParsedArn>`.

The two doc comments literally restate each other, and the two bodies re-derive the same three conditions (prefix, `splitn(6, ':')` field count, non-empty partition/service/resource) from scratch. Because the rule lives in two places it can drift: loosening or tightening one copy silently makes the tagging operations and the resource-set membership classifier disagree about whether the same string is an ARN at all — e.g. a `parse_arn` that accepted a five-field value would stop reporting `NOT_VALID_ARN` for it in `BatchAssociateResource` while `arn_format_error` still answered a 400 `ValidationException` for it in `TagResource`.

This is the same class of defect as the already-fixed `decode_account_page_token` / `decode_key_token` duplication: one rule, one place. The two copies can be collapsed (the per-field message `arn_format_error` produces can be built from the same single parse) with no change to any response or any existing test.

## Evidence

Copy 1 — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2176-2202, `arn_format_error`:

```
/// The form is the documented ARN layout
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`: the literal
/// `arn` prefix, six colon-separated fields, and a non-empty partition, service
/// and resource. Region and account id are deliberately allowed to be empty —
/// ARNs of global and account-less resources leave them so.
pub fn arn_format_error(arn: &str) -> Option<String> {
    if !arn.starts_with("arn:") { ... }
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() < 6 { ... }
    let empty = match (fields[1], fields[2], fields[5]) {
        ("", _, _) => Some("partition"),
        (_, "", _) => Some("service"),
        (_, _, "") => Some("resource"),
        _ => None,
    };
```

Copy 2 — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:2862-2882, `parse_arn`:

```
/// Parse an ARN, or `None` when `value` is not one at all: six colon-separated
/// fields, the first literally `arn`, and a non-empty partition, service and
/// resource. The region and account fields MAY be empty — a global service's ARN
/// leaves them so.
pub fn parse_arn(value: &str) -> Option<ParsedArn<'_>> {
    let mut fields = value.splitn(6, ':');
    if fields.next()? != "arn" { return None; }
    ...
    let complete = !parsed.partition.is_empty()
        && !parsed.service.is_empty()
        && !parsed.resource.is_empty();
    complete.then_some(parsed)
}
```

Both are reached from handlers of this crate: `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:182` — `if let Some(reason) = crate::state::arn_format_error(&input.resource_arn) { return Err(crate::scaffolded::Scaffolded::validation(reason)); }` (the same three lines appear in `untag_resource.rs:171` and `list_tags_for_resource.rs:181`), while `state.rs:2770` classifies a batch item with `let Some(arn) = parse_arn(uri) else { return Some("NOT_VALID_ARN") };`.

Existing tests that pin both sides and must stay green unchanged: the malformed-ARN `ValidationException` rows in `tests/all/tag_removal.rs:381`, `tests/all/tagging_administrator_gate.rs:226` and `tests/all/list_tags_across_administrator_states.rs:355`; the `NOT_VALID_ARN` rows in `tests/all/resource_set_disassociation.rs:139`, `tests/all/associable_discovered_resources.rs:390` and `tests/all/administrator_gated_operations.rs:910`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs and read `arn_format_error` (lines 2166-2202) and `parse_arn` (lines 2862-2882): both doc comments state the same rule (literal `arn` prefix, six colon-separated fields, non-empty partition/service/resource, region and account may be empty) and both bodies implement it independently.
2. Confirm both copies are live on handler paths: `grep -n arn_format_error src/ops` lists tag_resource.rs:182, untag_resource.rs:171, list_tags_for_resource.rs:181; `grep -n parse_arn src` lists state.rs:2770 (resource_set_item_failure_reason), state.rs:2998 (decode_uri_page_token), state.rs:3158 (in_region) and ops/list_resource_set_resources.rs:274.
3. Edit one copy only (for example make `parse_arn` tolerate a five-field value) and observe the divergence: `BatchAssociateResource` would stop reporting `NOT_VALID_ARN` for that string while `TagResource` still answers 400 `ValidationException` for it — nothing in the crate keeps the two in step.
