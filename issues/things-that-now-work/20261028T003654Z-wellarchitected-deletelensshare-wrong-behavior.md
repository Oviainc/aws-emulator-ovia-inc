Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2f325610dba287ab6eb0f722c6fb72ebe3ad3d74

# DeleteLensShare's idempotency fingerprint is spelling-sensitive, so a retry naming the same lens the other way is refused 409 instead of replaying the delete

_Filed: 2026-10-28T00:36:54Z_
_Operation: DeleteLensShare | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2f325610dba287ab6eb0f722c6fb72ebe3ad3d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteLensShare resolves the lens its `LensAlias` names through the crate's spelling-insensitive lens identity (`lens_by_alias`, which accepts the bare lens id, the lens ARN, and that ARN with a version pinned after it — all three name the SAME lens), but it builds the `@idempotencyToken` replay fingerprint over the RAW `input.lens_alias` string.

Consequence: after a delete under token T naming the lens by its plain ARN has completed successfully (200, empty body), the identical retry under token T that spells the lens as `<arn>/<version>` (or as the bare lens id) — the same lens, the same share, the same request — hashes to a different fingerprint, so `delete_replay` reports `Conflict` and the caller gets 409 ConflictException ("the token was already spent on a request with different parameters") instead of the replayed success the token exists to give. A success is turned into an error for a request the crate's own rule says is the same request.

The crate already owns the normalisation this path skips: `crate::state::lens_alias`. Its two sibling idempotent, lens-keyed operations call it inside the fingerprint for exactly this reason (UpgradeLensReview, UpgradeReviewTemplateLensReview); DeleteLensShare does not. The same spelling-sensitive fingerprint exists in `create_lens_share.rs:206-209` and `create_lens_version.rs:180-184`, but only DeleteLensShare's outcome diverges observably (409 instead of 200), so this issue is scoped to it.

## Evidence

Model rule the handler is implementing — `ClientRequestToken` (smithy_query query="shape" shape="ClientRequestToken"): "A unique case-sensitive string used to ensure that this request is idempotent (executes only once). … If you retry a request with the same client request token and the same parameters after the original request has completed successful[ly]…". `LensAlias` doc: "For custom lenses, this is the lens ARN… Each lens is identified by its LensSummary$LensAlias."

Side 1 — the fingerprint is over the raw spelling: `src/ops/delete_lens_share.rs:218-227` — "`ClientRequestToken` is `@idempotencyToken`, and the rule it documents holds here exactly as it does for `DeleteWorkload` and `DeleteReviewTemplate`: a retry under the same token and the same parameters … replays that request's result. The parameters besides the token are the lens the share belongs to and the share itself, so the fingerprint is over those two." followed by `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "LensAlias": &input.lens_alias, "ShareId": &input.share_id, }));`

Side 1b — yet the lens itself is resolved spelling-insensitively: `src/ops/delete_lens_share.rs:254-256` — `let lens_id = match store.lens_by_alias(&input.lens_alias).map(|record| record.lens_id.clone())`, and `src/state.rs:3967-3978` — "Whether `alias` names `record`: an ARN must agree with the lens's own ARN …, optionally pinning a version after it; a bare id names the lens whose id it is."

Side 2 — the sibling that applies the rule, with it stated: `src/ops/upgrade_lens_review.rs:212-224` — "The parameters besides the token are the three this request carries, with the lens taken as the review it names rather than as the spelling it was named by — a lens review is one review however the request spells its lens (`crate::state::lens_alias`), so a retry that names it the other way is the same request." and `"LensAlias": crate::state::lens_alias(&input.lens_alias),`. Same in `src/ops/upgrade_review_template_lens_review.rs:204-212`.

Side 2b — the crate asserts this spelling rule for THIS operation in its own test: `tests/all/lens_share_deletion.rs:14-16` — "A share is resolved against the lens its `LensAlias` NAMES, not against the spelling: a lens ARN and that ARN with a version pinned after it name the same lens", pinned by `tests/all/lens_share_deletion.rs:274-320` (`a_share_is_resolved_by_the_lens_its_name_resolves_to`, rows "created-plain-revoked-pinned" / "created-pinned-revoked-plain", both 200).

No test pins the divergent behaviour: the rule-3 rows above use a different `ClientRequestToken` per call (`share-{case}` / `revoke-{case}`), and the reused-token table `tests/all/lens_share_deletion.rs:548-632` (`a_retry_is_keyed_to_the_token_the_landed_delete_carried`) varies only same_token / same_lens / same_share, always spelling the retry's lens with the same `arn` string (`let retry_lens = if row.same_lens { arn } else { &other_lens };`). Normalising the fingerprint leaves every one of those rows unchanged.

## Steps to Reproduce

1. ImportLens a custom lens and CreateLensVersion to publish it, then CreateLensShare it with a principal to obtain a ShareId
2. DELETE /lenses/<plain lens ARN>/shares/<ShareId>?ClientRequestToken=tok-1 — answers 200 with an empty body and removes the share
3. Retry the identical request with the version-pinned spelling of the same lens that tests/all/lens_share_deletion.rs:274-320 proves names the same lens: DELETE /lenses/<plain lens ARN>/<version>/shares/<ShareId>?ClientRequestToken=tok-1
4. Observe 409 ConflictException (fingerprint mismatch at src/ops/delete_lens_share.rs:224-245) instead of the replayed 200 the `@idempotencyToken` rule quoted in that handler's own comment promises
5. Compare with src/ops/upgrade_lens_review.rs:220-224, which passes the alias through crate::state::lens_alias before fingerprinting so the equivalent retry replays
