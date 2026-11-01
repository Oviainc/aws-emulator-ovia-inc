Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: e4cbe047d5984815ac113e95db79725169e307b1

# The manager-not-found message echoes an unbounded ResourceArn, skipping the crate's own MAX_ECHOED_VALUE rule

_Filed: 2026-10-29T15:52:11Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `e4cbe047d5984815ac113e95db79725169e307b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::manager_not_found_message` interpolates the caller-supplied ARN into the `ResourceNotFoundException` message verbatim, with no bound on its length. Every other refusal message in this crate that echoes a caller-supplied value runs it through `crate::state::echoed_value` first, because the crate states a rule for exactly this: a caller-controlled value must not be echoed unelided. The three tag operations are the paths where that matters: `TagResource`, `UntagResource` and `ListTagsForResource` bind `ResourceArn` as a bare `@httpLabel` `String` with NO `@pattern` and NO `@length` (unlike `GetConfigurationManager`/`UpdateConfigurationManager`/`DeleteConfigurationManager`, whose `ManagerArn` pattern bounds the value to an ARN of about 100 characters), so arbitrary caller text of any length the request URI can carry reaches the handler and comes straight back inside the 404 body.

So one rule lives in two states inside the same module: `reserved_tag_key_rejection`, `quick_setup_type_rejection`, `resolve_type_version`, `deployment_targets_rejection` and `iam_role_arn_rejection` all elide, while `manager_not_found_message` — reached by six operations, three of them with an unconstrained input member — does not. A fix is behaviour-preserving for every real ARN: a configuration-manager ARN is far shorter than `MAX_ECHOED_VALUE` (256), so the fixed template `Quick setup manager with arn: <arn> could not be found.` that `tests/all/manager_not_found_message.rs` pins is unaffected; only an oversized value would be elided.

## Evidence

The rule the crate states — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1465-1470:
```
/// Longest caller-supplied value echoed in a refusal message.
///
/// The value is caller-controlled, so an unelided echo would amplify one request into
/// an arbitrarily long response. The same bound the framework's own constraint walk
/// applies.
const MAX_ECHOED_VALUE: usize = 256;
```
and the helper that applies it — state.rs:1474-1483:
```
fn echoed_value(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
```
Sibling paths that DO call it — state.rs:1530 (`reserved_tag_key_rejection`: "got '{}'", echoed_value(key)), state.rs:354 (`quick_setup_type_rejection`), state.rs:403 (`resolve_type_version`), state.rs:694 (`deployment_targets_rejection`), state.rs:1460 (`iam_role_arn_rejection`).

The path that skips it — state.rs:104-110:
```
/// text `Quick setup manager with arn: ` and ` could not be found.` around the ARN the
/// REQUEST carried, echoed verbatim.
pub fn manager_not_found_message(manager_arn: &str) -> String {
    format!("Quick setup manager with arn: {manager_arn} could not be found.")
}
```
The callers whose input member is unconstrained — src/ops/list_tags_for_resource.rs:168-176:
```
        let Some(manager) = store.manager_in(&region, &input.resource_arn) else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::manager_not_found_message(
                        &input.resource_arn,
```
and identically src/ops/tag_resource.rs:225-233 and src/ops/untag_resource.rs:226-234.

That the member is unbounded: `smithy_query(query="operation", operation="ListTagsForResource")` reports `"ResourceArn": {"target": "String", "httpLabel": true, "required": true}` — no `pattern`, no `length` (contrast `GetConfigurationManager`'s `"ManagerArn": {... "length": {"min": 1}, "pattern": "^arn:aws:ssm-quicksetup:...$"}`), and OPERATIONS.md:92 lists it as `ResourceArn` (string, @httpLabel, required) with no further constraint.

Test coverage: tests/all/manager_not_found_message.rs:10-16 pins the template for pattern-valid ARNs only ("a different ARN and expects that same ARN back inside the same fixed text"); no test sends an oversized `ResourceArn` to /tags/{ResourceArn}, so this path is unpinned.

## Steps to Reproduce

1. Boot the emulator and sign a request for ssm-quicksetup.
2. Call GET /tags/{ResourceArn} with a ResourceArn path segment of several thousand characters (the member carries no @pattern and no @length, so the constraint walk admits it and the handler runs).
3. Observe the 404 ResourceNotFoundException body: $.Message is `Quick setup manager with arn: <the whole multi-thousand-character value> could not be found.` — the caller's text is echoed unelided.
4. Repeat with PUT /tags/{ResourceArn} (TagResource) and DELETE /tags/{ResourceArn}?tagKeys=k (UntagResource): same unelided echo.
5. Compare with any refusal that goes through the crate's own rule, e.g. POST /configurationManager with a ConfigurationDefinitions[0].Type of 300 characters — state.rs:354 runs the value through echoed_value, so the message carries at most 256 characters plus the `... [N bytes]` elision.
