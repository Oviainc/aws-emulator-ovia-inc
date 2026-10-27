Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 22964030325ab1e634e5ef0028a831687ff48548

# Factor the duplicated single-ARN resource-filter rule shared by ListObjectives and ListCommonControls into one helper

_Filed: 2026-10-25T23:58:16Z_
_Operation: ListObjectives | Class: maintainability_

> _Note: this issue was identified against the code base at commit `22964030325ab1e634e5ef0028a831687ff48548`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a resource filter list carries at most one entry, that entry must name an ARN, and anything else is the service's one fixed invalid-filter sentence" is implemented twice, verbatim, in two handlers of this crate: `src/ops/list_objectives.rs` (for `ObjectiveFilter.Domains`) and `src/ops/list_common_controls.rs` (for `CommonControlFilter.Objectives`). The two blocks differ only in the member they read and the local they assign; the branch structure, the `> 1` comparison, the missing-`Arn` branch, the error variant and the message constant are identical, and both handler comments state the SAME rule and even say explicitly that the sentence "belongs to the class and not to the member at fault". Because the rule lives in two places, a later change to it (say, how an empty list or a duplicated ARN is treated) can be applied to one copy and not the other, and the two operations would then disagree about a rule the crate documents as being one rule. This crate already centralises every other cross-operation list rule in `src/state.rs` (`page`, `page_refusal`, `filter_fingerprint`, whose doc says it is built "the same way" by "[e]very list operation ... so the input to the hash is canonical across the service rather than hand-rolled per operation"), so one shared helper that normalises a single-ARN resource filter (returning `Option<String>` or the invalid-filter refusal) is the shape the crate already uses. The fix is a pure extraction: both operations keep returning the same status, the same `ValidationException` code and the same byte-identical message, and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs:195-218:
```
        let mut domain_filter: Option<String> = None;
        if let Some(filter) = input.objective_filter.as_ref() {
            if let Some(domains) = filter.domains.as_ref() {
                if domains.len() > 1 {
                    return Err(ListObjectivesErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(crate::service::INVALID_FILTER_MESSAGE.to_string()),
                        },
                    )
                    .into());
                }
                if let Some(entry) = domains.first() {
                    let Some(arn) = entry.arn.as_ref() else {
                        return Err(ListObjectivesErrorModeled::ValidationException(
                            ValidationException {
                                message: Some(crate::service::INVALID_FILTER_MESSAGE.to_string()),
                            },
                        )
                        .into());
                    };
                    domain_filter = Some(arn.clone());
                }
            }
        }
```

Copy 2 — services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs:204-227:
```
        let mut objective_filter: Option<String> = None;
        if let Some(filter) = input.common_control_filter.as_ref() {
            if let Some(objectives) = filter.objectives.as_ref() {
                if objectives.len() > 1 {
                    return Err(ListCommonControlsErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(crate::service::INVALID_FILTER_MESSAGE.to_string()),
                        },
                    )
                    .into());
                }
                if let Some(entry) = objectives.first() {
                    let Some(arn) = entry.arn.as_ref() else {
                        return Err(ListCommonControlsErrorModeled::ValidationException(
                            ValidationException {
                                message: Some(crate::service::INVALID_FILTER_MESSAGE.to_string()),
                            },
                        )
                        .into());
                    };
                    objective_filter = Some(arn.clone());
                }
            }
        }
```

Both handlers' own comments assert it is ONE rule: list_common_controls.rs:201-203 — "both are refused with the service's one fixed invalid-filter sentence — the same text ListObjectives answers its own filter with, because the sentence belongs to the class and not to the member at fault"; list_objectives.rs:192-194 — "both are refused with the service's one fixed invalid-filter sentence, which names neither the member nor the number of entries that were sent."

The crate's own precedent for factoring a cross-operation list rule — services/service-com-amazonaws-controlcatalog-controlcatalog/src/state.rs:336-339: "Every list operation builds it the same way ... so the input to the hash is canonical across the service rather than hand-rolled per operation."

Existing test coverage that must stay unchanged (and which pins the identical wording of both copies):
- services/service-com-amazonaws-controlcatalog-controlcatalog/tests/all/refusal_wording.rs:206 `an_unacceptable_filter_is_refused_with_the_one_fixed_sentence` — drives both `/objectives` and `/common-controls` with 2, 3 and N entries, duplicated ARNs, a published+unpublished pair, and entries naming no resource, and asserts at refusal_wording.rs:312-316 `sentences.len() == 1`, "both filter members must be refused with the same sentence".
- services/service-com-amazonaws-controlcatalog-controlcatalog/tests/all/control_catalog.rs:548 `a_single_entry_filter_is_empty_for_an_unknown_arn_and_refused_for_several`.
- services/service-com-amazonaws-controlcatalog-controlcatalog/tests/all/control_catalog.rs:453 `objectives_are_narrowed_to_the_domain_they_belong_to` and :497 `common_controls_are_narrowed_to_the_objective_they_belong_to` (the accepted single-ARN path).

## Steps to Reproduce

1. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_objectives.rs and read lines 195-218.
2. Open services/service-com-amazonaws-controlcatalog-controlcatalog/src/ops/list_common_controls.rs and read lines 204-227; the two blocks are the same branch structure, the same `> 1` check, the same missing-`Arn` branch and the same `crate::service::INVALID_FILTER_MESSAGE`, differing only in the filter member read and the local assigned.
3. Note that each handler's comment (list_objectives.rs:192-194, list_common_controls.rs:201-203) states the rule is ONE rule shared by both operations, while the code implements it twice, so editing one copy silently makes the two operations disagree.
4. Extract the rule into a single helper (e.g. in src/state.rs beside `page`/`filter_fingerprint`) returning either the one optional ARN or the invalid-filter refusal, and have both handlers call it.
5. Run the crate's test target: tests/all/refusal_wording.rs and tests/all/control_catalog.rs must pass unchanged — every status, error code and message byte stays the same.
