Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 008c7c502036a8aee2c1dc5905cbe39931fb487e

# DeleteLens's idempotency fingerprint is spelling-sensitive, so a retry naming the same lens the other way is refused 409 instead of replaying the delete

_Filed: 2026-10-28T23:35:22Z_
_Operation: DeleteLens | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `008c7c502036a8aee2c1dc5905cbe39931fb487e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteLens resolves the lens its `LensAlias` names through the crate's spelling-INSENSITIVE lens identity (`store.lens_by_alias`, which accepts the bare lens id, the lens ARN, and that ARN with a version pinned after it — all three name the SAME lens), but it builds the `@idempotencyToken` replay fingerprint over the RAW `input.lens_alias` string.

Consequence: after `DELETE /lenses/<lens ARN>?ClientRequestToken=T&LensStatus=ALL` has completed successfully (200, empty body), the identical retry under token T that spells the same lens the other way — the bare lens id, or the ARN with a version pinned after it — hashes to a different fingerprint, so `crate::state::delete_replay` answers `DeleteReplay::Conflict` and the caller gets 409 `ConflictException` ("the token was already spent on a request with different parameters") instead of the replayed success the token exists to give. The retry is the same request by the crate's own lens-identity rule, and a success is turned into an error.

The retry really does reach the ledger: `remove_lens` deliberately keeps `delete_lens_tokens` ("a token that names the delete itself has to survive the resource the delete removed, or the retry it was issued for could not be replayed"), so the only thing deciding between replay and 409 is the fingerprint.

The crate already owns the normalisation this path skips (`crate::state::lens_alias`), and two sibling idempotent lens-keyed operations call it inside the fingerprint for exactly this reason. Note this is a DIFFERENT operation from the already-filed DeleteLensShare instance of the same mistake; that issue explicitly scoped itself to DeleteLensShare and enumerated only `create_lens_share.rs` / `create_lens_version.rs` as the other spelling-sensitive fingerprints — it does not mention `delete_lens.rs`, whose outcome diverges observably in the same way (409 instead of 200).

## Evidence

Model rule the handler is implementing — `ClientRequestToken` doc (quoted in the handler itself, `src/ops/delete_lens.rs:226-231`): "a retry under the same token and the same parameters, after the original request completed successfully, replays that request's result. The parameters besides the token are the lens and the status, so the fingerprint is over both".

Side 1 — the fingerprint is over the RAW spelling: `src/ops/delete_lens.rs:232-235` — `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "LensAlias": &input.lens_alias, "LensStatus": &input.lens_status, }));` and `src/ops/delete_lens.rs:243-254` — `match crate::state::delete_replay(&store.delete_lens_tokens, &input.client_request_token, crate::state::LENS_RESOURCE_TYPE, &fingerprint) { … DeleteReplay::Conflict(exception) => return Err(…ConflictException(exception).into()), … }`.

Side 1b — yet the lens itself is resolved spelling-insensitively, two lines later: `src/ops/delete_lens.rs:261-265` — `let record = store.lens_by_alias(&input.lens_alias).ok_or_else(…)`, and `src/state.rs:4003-4014` — "Whether `alias` names `record`: an ARN must agree with the lens's own ARN (so the account and Region it carries have to match), optionally pinning a version after it; a bare id names the lens whose id it is."

Side 2 — the sibling that applies the rule, with the rule stated: `src/ops/upgrade_lens_review.rs:212-224` — "The parameters besides the token are the three this request carries, with the lens taken as the review it names rather than as the spelling it was named by — a lens review is one review however the request spells its lens (`crate::state::lens_alias`), so a retry that names it the other way is the same request." followed by `"LensAlias": crate::state::lens_alias(&input.lens_alias),`. Same in `src/ops/upgrade_review_template_lens_review.rs`.

Side 2b — the token really survives the delete, so the 409 is what a retry gets: `src/state.rs:7370-7372` — "[`Self::delete_lens_tokens`] is NOT among them … a token that names the delete itself has to survive the resource the delete removed, or the retry it was issued for could not be replayed."

Test coverage: the idempotency table `tests/all/custom_lens_deletion.rs:719-793` (`the_delete_is_idempotent_per_client_request_token`) varies only the STATUS and WHICH lens the retry names — `let target = if same_lens { &deleted.arn } else { &other.arn };` (line 749) — so every row spells the retry's lens with the same `arn` string as the original. Normalising the fingerprint through `crate::state::lens_alias` leaves all four rows (and the "different-lens" 409 rows, which name a different lens id) unchanged.

## Steps to Reproduce

1. ImportLens a custom lens and CreateLensVersion to publish a version of it; note the lens ARN and the bare lens id in it (`arn:aws:wellarchitected:<region>:<account>:lens/<lensId>`)
2. DELETE /lenses/<lens ARN>?ClientRequestToken=tok-1&LensStatus=ALL — answers 200 with an empty body and removes the lens
3. Retry the identical delete under the same token, spelling the lens the other way that `src/state.rs:4003-4014` says names the same lens: DELETE /lenses/<lensId>?ClientRequestToken=tok-1&LensStatus=ALL (or the version-pinned ARN form)
4. Observe 409 ConflictException from the fingerprint mismatch in `delete_replay` (`src/ops/delete_lens.rs:232-254`) instead of the replayed empty 200 the `@idempotencyToken` rule quoted in that handler's own comment promises
5. Compare with src/ops/upgrade_lens_review.rs:220-224, which passes the alias through `crate::state::lens_alias` before fingerprinting so the equivalent retry replays
