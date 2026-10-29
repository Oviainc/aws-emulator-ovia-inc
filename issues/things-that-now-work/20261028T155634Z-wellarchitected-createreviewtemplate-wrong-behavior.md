Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc8acfad318c0cd148284152a9b68532fd3f8c27

# CreateReviewTemplate stores its Lenses list without the lens-identity de-duplication UpdateReviewTemplate applies, so ReviewTemplate$Lenses reports one lens twice while the template holds one review

_Filed: 2026-10-28T15:56:34Z_
_Operation: CreateReviewTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc8acfad318c0cd148284152a9b68532fd3f8c27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateReviewTemplate` assigns the request's `Lenses` list to the stored template verbatim (`lenses: input.lenses`). `ReviewTemplateLenses` carries no `@uniqueItems`, and a lens is the same lens under several spellings (bare official alias, official lens ARN, version-pinned ARN) — the crate's own identity rule, `crate::state::lens_alias`.

Consequence: a create naming one lens twice stores TWO entries for it. `ReviewTemplate$Lenses` and `ReviewTemplateSummary$Lenses` are projected straight off that list, so `GetReviewTemplate` and `ListReviewTemplates` report the lens twice, while the template's lens reviews — keyed through `lens_reviews_of` by `lens_alias` — are exactly ONE, so `GetReviewTemplateLensReview` and the template's question roll-up see one lens. One stored template reports two lenses and one lens review at the same time.

The sibling operation that attaches lenses to the SAME resource de-duplicates before storing, with the rule spelled out: `UpdateReviewTemplate` keeps each `LensesToAssociate` entry once by `lens_alias`. `CreateReviewTemplate` does not, so which stored lens list a template ends up with depends on whether its lenses arrived through the create or through the update.

Secondary symptom of the same defect: the per-create lens quota at `create_review_template.rs:188` counts raw entries, so a `Lenses` list naming one lens eleven times is refused 402 `ServiceQuotaExceededException` although it applies one lens, while `update_review_template.rs:292` counts the DISTINCT lenses the template would end up applying.

## Evidence

Side 1 — the create stores the list raw: `src/ops/create_review_template.rs:307` `let lens_reviews = store.applied_lens_reviews(&input.lenses, now);` and `src/ops/create_review_template.rs:314` `lenses: input.lenses,` — no `lens_alias` pass anywhere in the file (the only quota check is `src/ops/create_review_template.rs:188` `if input.lenses.len() > crate::state::MAX_LENSES_PER_REVIEW_TEMPLATE`).

Side 1b — that raw list IS what a read reports: `src/state.rs:3101` `lenses: Some(self.lenses.clone()),` in `ReviewTemplateRecord::to_summary` and `src/state.rs:3122` `lenses: Some(self.lenses.clone()),` in `to_review_template`.

Side 1c — yet the template's reviews are de-duplicated by lens identity: `src/state.rs:3937-3940` — "every applied lens's [`lens_alias`] — the identity the lens is ONE lens under, so a lens applied under two spellings has exactly one review" — and `src/state.rs:3945-3947` says that one place serves both resources: "because a WORKLOAD's lens reviews ([`WorkloadRecord::lens_reviews`]) and a REVIEW TEMPLATE's ([`ReviewTemplateRecord::lens_reviews`]) are both the reviews of the lenses their resource applies, and the two cannot disagree about which lens is one review".

Side 2 — the sibling attach path on the same resource applies the de-duplication, with the rule stated: `src/ops/update_review_template.rs:174-187` — "The lenses the request applies, each kept once and in the order it first named them, AS SENT: a lens is one lens however it is spelled (`crate::state::lens_alias` resolves an alias and an ARN to the same lens), so a list naming one twice asks for ONE review." followed by `for lens in input.lenses_to_associate.iter().flatten() { if seen.insert(crate::state::lens_alias(lens)) { to_associate.push(lens.clone()); } }`; its quota check counts distinct lenses, `src/ops/update_review_template.rs:292` `if applied + fresh.len() > crate::state::MAX_LENSES_PER_REVIEW_TEMPLATE`, over the distinct `remaining` set built at `:249-258`.

Side 2b — the crate asserts the de-duplicating rule for the update in its own test: `tests/all/review_template_updates.rs:299` — "The same lens named twice asks for ONE review." Nothing pins duplicates surviving a create.

## Steps to Reproduce

1. POST /reviewTemplates with Lenses: ["wellarchitected", "arn:aws:wellarchitected:us-east-1::lens/wellarchitected"] (both spellings of the one framework lens), plus TemplateName, Description and a ClientRequestToken
2. GET /reviewTemplates/{TemplateArn} — ReviewTemplate$Lenses carries BOTH entries (two lenses), because src/ops/create_review_template.rs:314 stores input.lenses verbatim and src/state.rs:3122 projects it unchanged
3. GET /reviewTemplates/{TemplateArn}/lensReviews/wellarchitected — one review; the template's lens reviews are keyed by crate::state::lens_alias (src/state.rs:3948), so the lens list and the reviews disagree about how many lenses the template applies
4. Create a template with Lenses: ["wellarchitected"] and then PATCH /reviewTemplates/{TemplateArn} with LensesToAssociate: ["serverless", "arn:aws:wellarchitected:us-east-1::lens/serverless"]: the stored list gains ONE entry, per src/ops/update_review_template.rs:181-187
5. Also: POST /reviewTemplates with Lenses naming "wellarchitected" eleven times is refused 402 ServiceQuotaExceededException at src/ops/create_review_template.rs:188, although it applies one lens
