Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7f27151474f220c715589d81820913dcd4b7b62b

# The cross-region store-guard quota refusal formats the emulator's own cap into its message, which this crate's rule for that error class forbids

_Filed: 2026-11-07T21:28:20Z_
_Operation: CreateInvestigationGroup | Class: error-response_

> _Note: this issue was identified against the code base at commit `7f27151474f220c715589d81820913dcd4b7b62b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateInvestigationGroup can answer the modelled `ServiceQuotaExceededException` (402) from two helpers in `src/state.rs`, and the two word that one error class by opposite rules.

`region_quota_exceeded()` is written to a rule stated in its own doc comment: the sentence is fixed text and the emulator's cap constant must stay out of it, precisely so "the wording cannot become a function of emulator configuration". The crate's own end-to-end test restates the same rule for this error class.

`store_capacity_exceeded()` — ten lines below, reached from the same handler, answering the same modelled shape on the same operation — does the opposite: it `format!`s the emulator's `MAX_INVESTIGATION_GROUPS` constant straight into `message`. So one of this operation's two 402 sentences is a function of emulator configuration: change the guard constant and the wire message changes with it, while the sibling 402's sentence never does. A client that keys off the 402 message sees two different grammars from one operation, one of which leaks an emulator-internal number.

The cap itself is not in question here (its doc justifies it as an emulator resource guard); only the refusal WORDING, which the crate already decided must not carry the cap.

## Evidence

The rule, on the sibling helper — services/service-com-amazonaws-aiops-aiops/src/state.rs:1549-1557:
```
/// The quota is published on the operation itself: "Currently, you can have one
/// investigation group in each Region in your account." The MESSAGE, though, states
/// none of that: recorded behavior is the fixed sentence
/// [`CANNOT_CREATE_ANOTHER_GROUP`], which names no count, no Region and nothing the
/// request carried — so the cap this emulator enforces
/// ([`MAX_INVESTIGATION_GROUPS_PER_REGION`]) stays out of it and the wording cannot
/// become a function of emulator configuration.
pub fn region_quota_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(CANNOT_CREATE_ANOTHER_GROUP.to_string())
}
```

The violation — services/service-com-amazonaws-aiops-aiops/src/state.rs:1564-1570:
```
/// The 402 a create is refused with when the store has reached its cross-region
/// resource guard, [`MAX_INVESTIGATION_GROUPS`].
pub fn store_capacity_exceeded() -> ServiceQuotaExceededException {
    quota_exceeded(format!(
        "this account already holds the maximum of {MAX_INVESTIGATION_GROUPS} investigation groups"
    ))
}
```

The same rule, restated for this error class by the crate's own test — services/service-com-amazonaws-aiops-aiops/tests/all/service_worded_refusals.rs:18-19:
```
//! 2. the per-account/Region investigation-group quota — one fixed sentence, which
//!    draws nothing from the request and nothing from the cap the emulator enforces;
```

Both helpers feed the one `quota_exceeded` builder (src/state.rs:1537-1544) and both are returned by the same handler: services/service-com-amazonaws-aiops-aiops/src/ops/create_investigation_group.rs:318-337 returns `region_quota_exceeded()` then `store_capacity_exceeded()` as the same `CreateInvestigationGroupErrorModeled::ServiceQuotaExceededException`.

Test coverage of the affected path asserts only the status and the error code, never the message, so correcting the wording leaves every existing test passing — services/service-com-amazonaws-aiops-aiops/tests/all/investigation_groups.rs:1098-1106:
```
    assert_eq!(
        status, 402,
        "a create past the cap must be refused, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ServiceQuotaExceededException",
```
while the per-region sentence IS pinned verbatim (tests/all/service_worded_refusals.rs:61,380 and tests/all/investigation_group_singleton.rs:72: `"Cannot create another investigation group."`).

## Steps to Reproduce

1. Boot the emulator and point an aiops client at it.
2. Create one investigation group in each of MAX_INVESTIGATION_GROUPS (500) distinct signing regions, so the cross-region store guard is full.
3. Call CreateInvestigationGroup once more, signed for a fresh region that holds no group.
4. Observe 402 ServiceQuotaExceededException with Message == "this account already holds the maximum of 500 investigation groups" — a sentence that interpolates the emulator's own cap constant.
5. Compare with the sibling 402 on the same operation: with one group already in the signed-for region, CreateInvestigationGroup answers 402 with the fixed sentence "Cannot create another investigation group.", which interpolates nothing, per the rule quoted on region_quota_exceeded.
