Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# UpdateReviewTemplateLensReview writes a template's lens-review notes but never moves the review template's own UpdatedAt

_Filed: 2026-10-25T08:58:53Z_
_Operation: UpdateReviewTemplateLensReview | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateReviewTemplateLensReview` writes `LensNotes` / `PillarNotes` into the stored `ReviewTemplateLensReviewRecord` and stamps that record's `updated_at`, but it never stamps the OWNING `ReviewTemplateRecord::updated_at`. `ReviewTemplate$UpdatedAt` / `ReviewTemplateSummary$UpdatedAt` are projected straight off that field, so after a successful notes write `GetReviewTemplate` and `ListReviewTemplates` keep reporting the instant of the create (or of the last answer write) — a template whose content a caller just changed reports it was never updated.

This is inconsistent with the two sibling write paths of the SAME record in the SAME crate, both of which stamp both instants and both of which carry a comment stating that rule explicitly: `UpdateReviewTemplateAnswer` ("Answering a question changes the template and the lens review that asks it, so both report this write as the instant they were last updated") and `UpgradeReviewTemplateLensReview` ("The upgrade is a write to the lens review ... so both report this instant as the one they were last updated at — exactly as recording an answer against the template does"). Writing the template's lens notes is as much a write to the template as answering one of its questions is.

The analogous workload path does the same thing correctly: `UpdateLensReview` stamps `workload.updated_at` when it writes a lens review's notes.

## Evidence

Smithy model — `smithy_query(query="doc", shape="ReviewTemplate", member="UpdatedAt")`:
  "The date and time when the review template was last updated."

The handler that skips it — services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/update_review_template_lens_review.rs:238-250:
```
        if input.lens_notes.is_some() || input.pillar_notes.is_some() {
            let written = template.lens_review_record_mut(&input.lens_alias, now);
            if let Some(lens_notes) = input.lens_notes {
                written.notes = Some(lens_notes);
            }
            if let Some(pillar_notes) = input.pillar_notes {
                written.pillar_notes = pillar_notes.into_iter().collect();
            }
            written.updated_at = now;
        }
```
`grep -n 'updated_at' src/ops/update_review_template_lens_review.rs` returns exactly ONE line (249, `written.updated_at = now;`) — `template.updated_at` is never assigned.

Sibling #1 — src/ops/update_review_template_answer.rs:324-329:
```
        // Answering a question changes the template and the lens review that asks it, so
        // both report this write as the instant they were last updated.
        template
            .lens_review_record_mut(&input.lens_alias, now)
            .updated_at = now;
        template.updated_at = now;
```

Sibling #2 — src/ops/upgrade_review_template_lens_review.rs:289-297:
```
            let review = template.lens_review_record_mut(&input.lens_alias, now);
            review.lens_version = latest;
            // The upgrade is a write to the lens review, and through its version to the
            // template's own update status, so both report this instant as the one they
            // were last updated at — exactly as recording an answer against the template
            // does.
            review.updated_at = now;
            template.updated_at = now;
```

What the stale field is reported as — src/state.rs:2616-2627 (`ReviewTemplateRecord::to_review_template`, the projection `GetReviewTemplate` answers with):
```
    pub fn to_review_template(&self, update_status: &str) -> ReviewTemplate {
        ReviewTemplate {
            ...
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
```
and src/state.rs:2601-2606 (`to_summary`, what `ListReviewTemplates` answers with) reads the same field.

Test coverage: tests/all/review_template_lens_review.rs:756-821 (`updated_at_tracks_the_last_write`) asserts only the LENS REVIEW's `UpdatedAt` after this operation (`updated_at(&written, "written") >= template_updated_at`); no test asserts the TEMPLATE's `UpdatedAt` after `UpdateReviewTemplateLensReview`, so nothing pins the stale value.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. POST /reviewTemplates (CreateReviewTemplate) with Lenses=["wellarchitected"]; note TemplateArn
3. GET /reviewTemplates/{TemplateArn} (GetReviewTemplate) and record ReviewTemplate.UpdatedAt as T0
4. PATCH /reviewTemplates/{TemplateArn}/lensReviews/wellarchitected (UpdateReviewTemplateLensReview) with {"LensNotes":"a note"} — 200, and the returned LensReview.UpdatedAt is the write's instant T1 > T0
5. GET /reviewTemplates/{TemplateArn} again: ReviewTemplate.UpdatedAt is still T0, although the template's lens review was just rewritten
6. For contrast, PATCH .../answers/{QuestionId} (UpdateReviewTemplateAnswer) with any member: GetReviewTemplate now reports the new instant, because that handler stamps template.updated_at
