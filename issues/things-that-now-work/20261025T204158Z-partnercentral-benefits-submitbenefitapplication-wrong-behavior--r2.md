Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# A submit moves the application's status but leaves Revision unchanged, while the other two status transitions bump it

_Filed: 2026-10-25T20:41:58Z_
_Operation: SubmitBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has three caller-driven status transitions on a benefit application: SubmitBenefitApplication (PENDING_SUBMISSION -> IN_REVIEW), RecallBenefitApplication (-> PENDING_SUBMISSION) and CancelBenefitApplication (-> CANCELED). Cancel and Recall both call `BenefitApplication::revise`, which bumps `revision` and advances `UpdatedAt`; Submit calls `BenefitApplication::touch`, which advances only `UpdatedAt`. The rule stated on `touch` in state.rs ('a status transition the caller asked for records only the time') is therefore followed by exactly one of the three transitions and broken by the other two, so the crate has no single answer to 'does a status transition move the revision?'.

The observable consequence is on the optimistic-concurrency token itself: a caller that read the application before it was submitted still holds the stored `Revision` afterwards, so an AmendBenefitApplication (or UpdateBenefitApplication, for the statuses it accepts) built on that pre-submit read passes the revision check even though the stored record changed in between — whereas the identical sequence across a recall or a cancel is refused with ConflictException because those transitions did move the revision. Note this is a different gap from the already-fixed SubmitBenefitApplication/Stage issue (20261024T153423Z-partnercentral-benefits-submitbenefitapplication-wrong-behavior), which concerned the `Stage` member and not the revision.

## Evidence

The rule, state.rs:1030-1038:
```
/// Advance `UpdatedAt` past the previous write, without changing the revision.
///
/// The revision is the optimistic-concurrency token `UpdateBenefitApplication`
/// checks, so only a change to the application's own content moves it (see
/// [`BenefitApplication::revise`]); a status transition the caller asked for records
/// only the time.
pub fn touch(&mut self, now_millis: i64) {
```

The handler that follows it — src/ops/submit_benefit_application.rs:200-201:
```
        application.set_status(crate::state::STATUS_IN_REVIEW, None);
        application.touch(now);
```

The two sibling status transitions that break it — src/ops/cancel_benefit_application.rs:224-225:
```
        application.set_status(crate::state::STATUS_CANCELED, input.reason);
        application.revise(now);
```
and src/ops/recall_benefit_application.rs:220-224:
```
        application.set_status(
            crate::state::STATUS_PENDING_SUBMISSION,
            Some(input.reason),
        );
        application.revise(now);
```
with state.rs:1098-1102 showing `revise` bumps the revision:
```
    pub fn revise(&mut self, now_millis: i64) {
        self.recall_client_token = None;
        self.revision = self.revision.saturating_add(1);
        self.touch(now_millis);
    }
```

What the revision is for, from the model: `AmendBenefitApplicationInput$Revision` — 'The current revision number of the benefit application to ensure optimistic concurrency control.' (smithy_query doc, shape=AmendBenefitApplicationInput, member=Revision). The revision check it feeds is src/ops/amend_benefit_application.rs:533 `if application.revision_text() != input.revision {`.

Test coverage: the submit test asserts only the status and that `UpdatedAt` advanced — tests/all/benefit_application_lifecycle.rs:1095-1103 (`submit_transitions_only_from_pending_submission`), and tests/all/application_workflow_stage.rs:261-262 only checks UpdatedAt does not move backwards; nothing pins the revision across a submit. The sibling behaviour IS pinned: tests/all/benefit_application_lifecycle.rs:1238-1240 ('a cancel must bump the revision') and tests/all/recall_benefit_application.rs:158-160 ('a recall must advance the revision').

## Steps to Reproduce

1. Start the emulator with the partner entitlement granted, and CreateBenefitApplication in Catalog=AWS against an ACTIVE benefit.
2. GetBenefitApplication on the new Id and note Revision (R1) and Status=PENDING_SUBMISSION.
3. SubmitBenefitApplication for that Id.
4. GetBenefitApplication again: Status is IN_REVIEW and UpdatedAt has advanced, but Revision is still R1 — the stored record changed while the concurrency token did not.
5. AmendBenefitApplication for that Id carrying Revision=R1 (the value read BEFORE the submit): it is accepted, because the revision never moved.
6. Compare with the other two transitions: CancelBenefitApplication or RecallBenefitApplication on a comparable application does bump Revision (pinned by tests/all/benefit_application_lifecycle.rs:1238 and tests/all/recall_benefit_application.rs:158), so a pre-transition revision is refused there.
