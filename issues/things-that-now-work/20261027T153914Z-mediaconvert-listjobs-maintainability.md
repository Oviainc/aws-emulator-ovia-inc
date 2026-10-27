Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0d4ec79976a0b91eeb5a23059b2beecdecae287a

# The "decode nextToken or refuse it" rule and its refusal sentence are written out seven times in seven handlers

_Filed: 2026-10-27T15:39:14Z_
_Operation: ListJobs | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0d4ec79976a0b91eeb5a23059b2beecdecae287a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged reader in this crate repeats the same seven-line rule verbatim: decode the caller's `nextToken` with `crate::state::decode_cursor`, and on `None` return the operation's `BadRequestException` carrying the literal sentence "nextToken is not a token this service issued.". There are seven copies, and the sentence is a bare string literal in each one.

That is the one refusal sentence in this crate that does NOT live in `state.rs`. Every other refusal this service words is a `state::*_message()` function precisely so the copies cannot drift — the crate says so in its own comments — and the state module already owns the rest of the paging rule (`decode_cursor`, `encode_cursor`, `page_of_records`, `DEFAULT_PAGE_SIZE`). The divergence is not hypothetical: `DescribeEndpoints`, the eighth handler that refuses a `nextToken`, already words the same refusal differently.

The fix is to move the decode-or-refuse rule (or at minimum the sentence) into `state.rs` beside `decode_cursor`, and have each handler map the one refusal onto its own modelled `BadRequestException`. Every response and every existing test stays byte-identical.

## Evidence

Seven copies of the same branch and the same literal:

`src/ops/list_jobs.rs:186-197`
```rust
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => {
                let Some(anchor) = crate::state::decode_cursor(token) else {
                    return Err(ListJobsErrorModeled::BadRequestException(BadRequestException {
                        message: Some("nextToken is not a token this service issued.".to_string()),
                    })
                    .into());
                };
                Some(anchor)
            }
        };
```
and the same block at `src/ops/search_jobs.rs:196-211`, `src/ops/list_queues.rs:182-197`, `src/ops/list_presets.rs:173-188`, `src/ops/list_job_templates.rs:182-197`, `src/ops/list_versions.rs:171-186`, `src/ops/start_jobs_query.rs:232-247`.

The convention the crate states for exactly this, `src/ops/update_queue.rs:235-237`:
```rust
                    // The same sentence `CreateQueue` answers for the same pairing: it
                    // lives in `state` so the two refusals cannot drift.
                    message: Some(crate::state::reservation_plan_not_applicable_message()),
```
and `src/state.rs:703-706`: "so both the create and the update that carry one for an on-demand queue state the same refusal; the sentence lives here so the two cannot drift." The same pattern is used for `not_found_message`, `blank_member_message`, `empty_tag_key_message` (`src/state.rs:1461-1464`), `tags_past_cap_message`, `invalid_job_engine_version_message`, `hop_destination_is_source_message`, `policy_allows_no_input_message`.

The copies have already drifted — `src/ops/describe_endpoints.rs:198-207` words the same refusal with its own sentence:
```rust
        if let Some(token) = input.next_token.as_deref() {
            return Err(DescribeEndpointsErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(format!(
                        "nextToken is not a token this service issued: {token:?}."
                    )),
                },
```

Test coverage that must stay unchanged — `tests/all/shared_paging.rs:487-528`, which asserts the single sentence across every listing and calls it "the shared refusal message":
```rust
async fn every_listing_refuses_a_next_token_it_did_not_issue() {
...
                Some("nextToken is not a token this service issued."),
                "case {case}: expected the shared refusal message, got {}",
```

## Steps to Reproduce

1. Open src/ops/list_jobs.rs:186-197 and src/ops/list_queues.rs:182-197 — the two blocks are identical apart from the error enum name, including the refusal literal.
2. Repeat for src/ops/search_jobs.rs:196-211, src/ops/list_presets.rs:173-188, src/ops/list_job_templates.rs:182-197, src/ops/list_versions.rs:171-186 and src/ops/start_jobs_query.rs:232-247 — seven copies in total.
3. grep the crate for the literal "nextToken is not a token this service issued": it appears in seven handlers and nowhere in src/state.rs, unlike every other refusal sentence this service words.
4. Compare src/ops/describe_endpoints.rs:198-207, which already words the same refusal differently — the drift the shared-sentence convention exists to prevent.
5. Run `tests/all/shared_paging.rs::every_listing_refuses_a_next_token_it_did_not_issue`: it pins the one sentence for every listing, so hoisting it changes no response.
