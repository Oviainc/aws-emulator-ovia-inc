Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0484a4a393e00c66ab2f2d9da27b0764a19da5b

# CreateLensShare's idempotency fingerprint is spelling-sensitive, so a retry naming the same lens the other way is refused 409 instead of replaying the share

_Filed: 2026-10-29T00:55:34Z_
_Operation: CreateLensShare | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0484a4a393e00c66ab2f2d9da27b0764a19da5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateLensShare resolves the lens its `LensAlias` names through the crate's spelling-insensitive lens identity (`store.lens_by_alias`, which accepts the lens ARN AND that ARN with a version pinned after it as naming the SAME lens), but it builds the `@idempotencyToken` replay fingerprint over the RAW `input.lens_alias` string.

Consequence: after a share under token T that named the lens by its plain ARN has completed successfully (200 with the minted `ShareId`), the identical retry under token T that spells the lens as `<arn>/<version>` — the same lens, the same principal, the same request — hashes to a different fingerprint, so the handler takes the mismatch branch and answers 409 `ConflictException` ("the token was already spent on a request with different parameters") instead of the replayed 200 carrying the original `ShareId`. The token exists precisely to make that retry a replay; here a success is turned into an error for a request the crate's own lens-identity rule says is the same request.

The crate already owns the normalisation this path skips: `crate::state::lens_alias`. Its two sibling idempotent, lens-keyed operations call it INSIDE the fingerprint and state why (UpgradeLensReview, UpgradeReviewTemplateLensReview); CreateLensShare does not. Normalising the fingerprint changes nothing for any request that spells the lens consistently, so every existing row of `tests/all/lens_shares.rs` stays green.

(The open issue 20261028T003654Z-wellarchitected-deletelensshare-wrong-behavior scopes itself to DeleteLensShare and asserts that CreateLensShare's identical fingerprint does not "diverge observably" because a duplicate share is a 409 anyway. That reasoning compares the defect against the NO-token path: with the fingerprint normalised the retry is answered 200 with the original ShareId, so the divergence from the documented replay is observable here too.)

## Evidence

Model rule the handler is implementing — `CreateLensShareInput$ClientRequestToken`, quoted in the handler's own header at `src/ops/create_lens_share.rs:70-78`: "A unique case-sensitive string used to ensure that this request is idempotent (executes only once). You should not reuse the same token for other requests. If you retry a request with the same client request token and the same parameters after the original request has completed successfully, the result of the original request is returned."

Side 1 — the fingerprint is over the raw spelling: `src/ops/create_lens_share.rs:206-209` — `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "LensAlias": &input.lens_alias, "SharedWith": &input.shared_with, }));` and the mismatch branch it feeds, `src/ops/create_lens_share.rs:216-230` — `if token.fingerprint != fingerprint { ... return Err(CreateLensShareErrorModeled::ConflictException(crate::state::idempotency_token_conflict(...)).into()); }` while the matching branch is the replay, `:231-235` — `if let Some(recorded) = store.lens_share(&token.lens_id, &token.share_id) { return Ok(CreateLensShareOutput { share_id: Some(recorded.share_id.clone()) }); }`.

Side 1b — yet the lens itself is resolved spelling-insensitively on the very next lines: `src/ops/create_lens_share.rs:242` — `let Some(record) = store.lens_by_alias(&input.lens_alias) else {`, over `src/state.rs:4003-4014` — "Whether `alias` names `record`: an ARN must agree with the lens's own ARN (so the account and Region it carries have to match), optionally pinning a version after it; a bare id names the lens whose id it is" / `alias == record.lens_arn || alias.strip_prefix(record.lens_arn.as_str()).is_some_and(|version| version.starts_with('/'))`, and `src/state.rs:3991-4001` — "A lens ARN may also pin a version after the alias, which names the same lens." A version-pinned custom-lens ARN also passes this operation's official-lens guard, because `is_aws_official_lens` keys on the ARN's account field (`src/state.rs:4025-4032`).

Side 2 — the sibling that applies the normalisation, with the rule stated: `src/ops/upgrade_lens_review.rs:212-224` — "The parameters besides the token are the three this request carries, with the lens taken as the review it names rather than as the spelling it was named by — a lens review is one review however the request spells its lens (`crate::state::lens_alias`), so a retry that names it the other way is the same request." followed by `"LensAlias": crate::state::lens_alias(&input.lens_alias),`. Same in `src/ops/upgrade_review_template_lens_review.rs:204-212`.

Test coverage: no row pins the divergent behaviour. `tests/all/lens_shares.rs:552-562` ("The SAME token with DIFFERENT parameters is not a replay") varies only `SharedWith` (`create_share(&client, &arn, "777788889999", &token)`), and the replay row `tests/all/lens_shares.rs:335-342` re-sends the same `arn` string, so normalising the fingerprint leaves both unchanged. The crate asserts the spelling rule for the sibling delete in `tests/all/lens_share_deletion.rs:14-16` — "A share is resolved against the lens its `LensAlias` NAMES, not against the spelling: a lens ARN and that ARN with a version pinned after it name the same lens".

## Steps to Reproduce

1. ImportLens a custom lens, wait for the import to land, and CreateLensVersion to publish it (the ARN and version are what the create answers)
2. POST /lenses/<plain lens ARN>/shares with {"SharedWith": "111122223333", "ClientRequestToken": "tok-1"} — answers 200 with a minted ShareId
3. Retry the identical request spelling the lens the way state.rs:4003-4014 says names the same lens: POST /lenses/<plain lens ARN>/<published version>/shares with the SAME body and the SAME ClientRequestToken "tok-1"
4. Observe 409 ConflictException from the fingerprint-mismatch branch at src/ops/create_lens_share.rs:216-230, instead of the replayed 200 carrying the original ShareId that the ClientRequestToken doc quoted at src/ops/create_lens_share.rs:70-78 promises
5. Compare with src/ops/upgrade_lens_review.rs:220-224, which passes the alias through crate::state::lens_alias before fingerprinting so the equivalent retry replays
