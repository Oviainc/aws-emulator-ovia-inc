Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f7e63039319d8a0111addfa90cab83fc14fd19d7

# The header-block ceiling walk is written out twice (oversize_header_block and header_fields), and the two copies have already drifted

_Filed: 2026-10-27T20:50:07Z_
_Operation: PutRawMessageContent | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f7e63039319d8a0111addfa90cab83fc14fd19d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutRawMessageContent` bounds how much of a message's HEADER BLOCK it reads with two constants, `MAX_HEADER_BLOCK_OCTETS` and `MAX_HEADER_FIELDS`. The walk that applies them — split the content on LF, stop at the first empty line, accumulate each line's octets plus the LF that ended it, and stop once a ceiling is crossed — is implemented twice in the same file: once in `oversize_header_block`, which decides the refusal without allocating, and once again inside `header_fields`, which re-walks the very same block and re-applies the very same two ceilings while building the fields.

The two copies are not the same walk any more, which is exactly the drift this class is about:

1. the counting ceiling differs — `oversize_header_block` refuses on `lines > MAX_HEADER_FIELDS` (every line of the block, including folded continuations and lines that carry no field), while `header_fields` stops on `fields.len() >= MAX_HEADER_FIELDS` (only lines that parsed into a field). The two quantities are different for any block that folds a value;
2. the octet accounting differs — `oversize_header_block` returns at the terminating empty line BEFORE adding that line's octets, while `header_fields` adds the line's octets and tests the ceiling BEFORE it recognises the line as the end of the block, so the same block is one or two octets "longer" in the second copy than in the first.

Today those differences cancel out (`validate_mime` calls `oversize_header_block` first and returns `Err` before `header_fields` runs, and the second copy's early stop lands on the empty line that would have ended the walk anyway), so no response changes. That is precisely why this is a maintainability defect rather than a behaviour one: one ceiling rule is stated in two places that already disagree in detail, and any future change to either ceiling, to what counts as the end of the block, or to how a line's octets are counted has to be made in both to keep them cancelling.

The fix is to state the walk once — a single helper that yields the bounded header block (or the refusal when it is past a ceiling) and have `header_fields` parse what that helper hands back — leaving every response and every existing test unchanged.

## Evidence

Copy 1, the non-allocating pre-check — services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs:679-701:
```rust
fn oversize_header_block(content: &[u8]) -> Option<String> {
    let mut octets = 0usize;
    let mut lines = 0usize;
    for line in content.split(|byte| *byte == b'\n') {
        if line.strip_suffix(b"\r").unwrap_or(line).is_empty() {
            // The end of the header block.
            return None;
        }
        // The LF that ended the line belongs to the block as well.
        octets = octets.saturating_add(line.len()).saturating_add(1);
        lines += 1;
        if octets > MAX_HEADER_BLOCK_OCTETS { ... }
        if lines > MAX_HEADER_FIELDS { ... }
```

Copy 2, the parse — same file, put_raw_message_content.rs:799-812:
```rust
fn header_fields(content: &[u8]) -> HeaderBlock {
    let mut fields: HeaderBlock = Vec::new();
    let mut octets = 0usize;
    for line in content.split(|byte| *byte == b'\n') {
        // The LF that ended the line belongs to the block as well.
        octets = octets.saturating_add(line.len()).saturating_add(1);
        if octets > MAX_HEADER_BLOCK_OCTETS || fields.len() >= MAX_HEADER_FIELDS {
            break;
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            // The end of the header block.
            break;
        }
```
Same comment ("The LF that ended the line belongs to the block as well.", lines 687 and 803), same two constants (put_raw_message_content.rs:551 `const MAX_HEADER_BLOCK_OCTETS: usize = 1024 * 1024;` and :555 `const MAX_HEADER_FIELDS: usize = 10_000;`), two different stopping rules (`lines > MAX_HEADER_FIELDS` at :696 vs `fields.len() >= MAX_HEADER_FIELDS` at :805) and two different placements of the empty-line test relative to the octet accumulation (:683-688 vs :804-809).

The constant's own doc already names the quantity only one of the copies counts — put_raw_message_content.rs:553-555: "How many header lines this emulator will read out of one header block, the other half of the guard [`MAX_HEADER_BLOCK_OCTETS`] states." — while `header_fields` applies it to FIELDS, not lines.

Test coverage that pins the behaviour a dedupe must not change: tests/all/header_block_ceiling.rs:120 `a_header_block_is_read_up_to_the_ceiling_and_refused_past_it` (rows at `MAX_HEADER_BLOCK_OCTETS - 1`, exactly at it, `+ 1`, `* 4`, and `block_of_lines(MAX_HEADER_FIELDS - 1 / MAX_HEADER_FIELDS / + 1 / * 4)`), tests/all/header_block_ceiling.rs:211 `a_block_past_the_ceiling_is_refused_though_its_fields_are_in_order`, plus the field-parsing tests tests/all/mime_header_criteria.rs:113 and tests/all/mime_header_octets.rs:331 `a_header_field_is_the_field_its_name_octets_name` and :445 `a_unique_header_is_counted_by_its_name_octets`, which exercise `header_fields` through the wire.

## Steps to Reproduce

1. Open services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs and read `oversize_header_block` (lines 679-704) and `header_fields` (lines 799-828).
2. Observe both walk `content.split(|byte| *byte == b'\n')` to the first empty line, both accumulate `line.len() + 1` octets per line with the same comment, and both apply `MAX_HEADER_BLOCK_OCTETS` and `MAX_HEADER_FIELDS`.
3. Observe the two copies disagree on what the second ceiling counts: `lines += 1; if lines > MAX_HEADER_FIELDS` (line 689/696) against `if ... || fields.len() >= MAX_HEADER_FIELDS` (line 805), which differ for any block that folds a header value.
4. Observe they also disagree on whether the terminating empty line's octets count towards `MAX_HEADER_BLOCK_OCTETS`: the first returns at line 683 before accumulating, the second accumulates at line 804 and tests the ceiling at 805 before recognising the empty line at 809.
5. Confirm the behaviour that must stay unchanged is pinned by tests/all/header_block_ceiling.rs:120 and :211, which drive blocks one octet/one line inside, exactly at, and past both ceilings over the wire.
