Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8d31dcdeed45f0913eecb26aedcf59e0f3c47424

# The paged-walk rule (resume strictly after the cursor, over-fetch one, truncate, mint the token from the last kept key) is written out twice, once per Describe handler

_Filed: 2026-10-28T15:46:58Z_
_Operation: DescribeScalingPlans | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8d31dcdeed45f0913eecb26aedcf59e0f3c47424`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both paginated reads implement the SAME page rule in their own handler body: filter to entries whose key is strictly greater than the decoded cursor, take `page_size + 1`, and if more than a page came back truncate to `page_size` and mint the next token from the last kept entry's key. Only the cursor VALUE helpers were centralised (`crate::state::encode_cursor` / `decode_cursor` / `effective_max_results`); the walk itself is duplicated, so the two copies can drift — a change to how "is there another page?" is decided, or to the strict-greater-than resume, has to be made twice to keep the two operations' paging identical. `DescribeScalingPlanResources` even states that it is re-implementing its sibling's rule rather than calling it.

A single generic helper in `state.rs` (taking an ordered iterator, the decoded cursor, the page size and a key accessor, returning the page plus the optional token) would hold the rule once. Every response and every existing paging test stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plans.rs:231-244:
```
        let mut page: Vec<&crate::state::ScalingPlanRecord> = matching
            .filter(|plan| after.as_ref().is_none_or(|last| plan.name > *last))
            .take(page_size + 1)
            .collect();

        // One record more than the page was taken, so "is there another page?" is
        // answered from state rather than guessed.
        let next_token = if page.len() > page_size {
            page.truncate(page_size);
            page.last()
                .map(|plan| crate::state::encode_cursor(CURSOR_KIND, &plan.name))
        } else {
            None
        };
```

Copy 2 — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plan_resources.rs:230-248, whose own comment admits it is the sibling's rule:
```
                    // One resource more than the page is taken, so "is there another
                    // page?" is answered from state rather than guessed — the same way
                    // the sibling read over the plans themselves answers it.
                    let mut page: Vec<(String, usize, &ScalingInstruction)> = covered
                        .into_iter()
                        .filter(|(identity, _, _)| {
                            after.as_ref().is_none_or(|last| identity > last)
                        })
                        .take(page_size + 1)
                        .collect();
                    let next_token = if page.len() > page_size {
                        page.truncate(page_size);
                        page.last().map(|(identity, _, _)| {
                            crate::state::encode_cursor(CURSOR_KIND, identity)
                        })
                    } else {
                        None
                    };
```

The crate already declares `state.rs` the owner of the shared paging logic — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:5-8:
```
//! The whole service is one resource — the scaling plan — so this module owns the
//! stored record, the lifecycle clock that moves a plan out of its non-terminal
//! status, and the two value helpers (timestamps and pagination cursors) that more
//! than one handler in `src/ops/` reaches.
```
with `encode_cursor` at state.rs:1504, `decode_cursor` at state.rs:1512 and `effective_max_results` at state.rs:1525 — the value helpers both handlers DO share.

Tests that cover both copies and must stay unchanged: tests/all/scaling_plan_lifecycle.rs:1025-1080 (walks DescribeScalingPlans with MaxResults=2 and DescribeScalingPlanResources with MaxResults=1 page by page) and :1090-1123 (the MaxResults bounds and the InvalidNextTokenException row for both operations), tests/all/scaling_plan_resource_paging.rs:129-168 and :412-414, tests/all/scaling_plan_resource_policies.rs:313-349.

## Steps to Reproduce

1. Read services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plans.rs:231-244.
2. Read services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/describe_scaling_plan_resources.rs:230-248 and note the identical over-fetch-one / truncate / mint-from-last structure and the comment "the same way the sibling read over the plans themselves answers it".
3. Note that state.rs centralises only encode_cursor/decode_cursor/effective_max_results (state.rs:1504, 1512, 1525), so a change to the walk itself must be applied in both handlers.
4. Run the crate's paging tests (tests/all/scaling_plan_lifecycle.rs, tests/all/scaling_plan_resource_paging.rs, tests/all/scaling_plan_resource_policies.rs) before and after folding the walk into one helper: the responses and the assertions are unchanged.
