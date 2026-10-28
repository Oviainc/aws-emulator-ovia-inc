Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# Region-segment rule is written twice in state.rs: regex in RESOURCE_ARN_PATTERN and hand-rolled in is_region_shaped

_Filed: 2026-10-28T02:36:57Z_
_Operation: CreateChallenge | Class: maintainability_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate expresses ONE rule — the shape this service's ARNs require of their Region segment, `[a-z]+(-[a-z]+)+-[1-9]\d*` — in two independent implementations inside `src/state.rs`:

1. as a regex fragment embedded in `RESOURCE_ARN_PATTERN`, which the crate feeds to the framework's own constraint machinery (`PatternTrait` + `validate`) to refuse a malformed `ResourceArn`; and
2. as a hand-rolled ~24-line byte walk in `is_region_shaped`, used by `request_region` (which every handler calls, e.g. `create_challenge.rs:180`) to decide which Region a record is filed under.

The two copies must agree: the Region `request_region` accepts is the Region `connector_arn`/`challenge_arn` then embed in an ARN that `RESOURCE_ARN_PATTERN` has to accept back. Because the rule is spelled once as a regex and once as imperative code, they can drift independently — a change to the model's Region segment (or a correction to one copy) silently updates only one, letting the service file a record under a Region whose own ARN its tagging path would then refuse, or vice versa. The crate already depends on `aws_smithy_schema_server::constraints::{validate, PatternTrait}` (imported at `state.rs:18`) and already uses exactly that machinery for the other pattern, so the second, hand-rolled copy is replaceable by the pattern check the crate already performs, with the extra `MAX_REGION_LENGTH` bound kept as the separate bound it is.

The fix is behaviour-preserving: the accept/reject set is identical (`is_region_shaped`'s doc states the same expression it implements), so every response and every existing test stays unchanged.

## Evidence

Copy 1 — the rule as a regex, `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs:573-577`:
```rust
pub const RESOURCE_ARN_PATTERN: &str = concat!(
    r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}",
    r":connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}",
    r"(\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12})?$"
);
```
and the machinery that evaluates it, `state.rs:593` and `state.rs:688`:
```rust
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
...
    let violations = validate(&TaggedResource { resource_arn }, &TAGGED_RESOURCE_SCHEMA);
```
with the crate already importing that machinery at `state.rs:18`:
```rust
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
```

Copy 2 — the SAME rule hand-rolled, `state.rs:793-820` (its own doc names the expression it re-implements):
```rust
/// Whether `value` has the shape every ARN of this service requires of its Region segment:
/// `[a-z]+(-[a-z]+)+-[1-9]\d*` (smithy_query shape=ConnectorArn), within
/// [`MAX_REGION_LENGTH`] — two or more lower-case words joined by `-`, then a number with no
/// leading zero.
pub fn is_region_shaped(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_REGION_LENGTH {
        return false;
    }
    let Some((words, number)) = value.rsplit_once('-') else {
        return false;
    };
    if !number
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
        || !number.bytes().all(|byte| byte.is_ascii_digit())
    { return false; }
    let mut word_count = 0usize;
    for word in words.split('-') {
        if word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_lowercase()) {
            return false;
        }
        word_count += 1;
    }
    word_count >= 2
}
```
Used from `state.rs:789` inside `request_region`, which every handler calls (e.g. `src/ops/create_challenge.rs:180`: `let region = crate::state::request_region(ctx);`).

Test coverage that must stay green (both sides are already pinned end-to-end):
- `tests/all/region_isolation.rs:715-772` — `a_scope_naming_no_region_of_this_service_is_served_in_the_default_region`, whose own doc quotes the duplicated rule ("Rows are the ways a scope's Region field can fail the shape both ARN patterns require (`[a-z]+(-[a-z]+)+-[1-9]\d*`, smithy_query shape=ConnectorArn)") and drives the reject set (`"US-EAST-1"`, `"useast1"`, `"us-east"`, `"us-east-0"`, `"us-east-01"`, `"us--east-1"`, `"us-east1-x"`, `"us-e@st-1"`, an overlong name).
- `tests/all/resource_arn_validation.rs:64-68` and `:275` — the refusal that quotes `RESOURCE_ARN_PATTERN` back (`format!("Member must satisfy regular expression pattern: {RESOURCE_ARN_PATTERN}")`).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs` and read `RESOURCE_ARN_PATTERN` (lines 573-577): the Region segment of the ARN rule is the regex `[a-z]+(-[a-z]+)+-[1-9]\d*`.
2. Read `is_region_shaped` (lines 793-820): the same expression, named in its doc comment, re-implemented as a byte walk (`rsplit_once('-')`, digit check with no leading zero, lower-case word count >= 2).
3. Observe that the crate already evaluates the first copy with framework machinery it imports at line 18 (`PatternTrait`, `validate`) and applies at lines 593 and 688, so the second copy is a hand-rolled version of a check the crate already has.
4. Change the Region segment in one copy only (e.g. permit a single-word Region in `RESOURCE_ARN_PATTERN`) and note nothing forces the other copy to follow: `request_region` and the `ResourceArn` refusal now disagree about which Regions this service can name, while `cargo test -p service-com-amazonaws-pcaconnectorscep-pcaconnectorscep` reports the drift only through whichever suite happens to touch that copy.
5. Fix direction: derive both from one Region-segment constant, evaluating it through the `PatternTrait`/`validate` path the crate already uses, keeping `MAX_REGION_LENGTH` as its own separate bound; responses and all existing tests stay unchanged.
