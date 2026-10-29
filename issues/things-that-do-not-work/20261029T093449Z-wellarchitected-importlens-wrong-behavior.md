Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f

# ImportLens's idempotency fingerprint is spelling-sensitive, so a retry naming the same custom lens the other way is refused 409 instead of replaying the import

_Filed: 2026-10-29T09:34:49Z_
_Operation: ImportLens | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d3501a26036a1ce5fc0e6dafb00a9517af2aaa8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ImportLens resolves the lens its `LensAlias` names through the crate's spelling-INSENSITIVE lens identity (`store.lens_by_alias_mut`, which accepts the lens ARN and that ARN with a version pinned after it as naming the SAME lens), but it builds the `@idempotencyToken` replay fingerprint over the RAW `input.lens_alias` string.

Consequence: after `PUT /importLens` with `ClientRequestToken=T` and `LensAlias=<the lens ARN>` has completed successfully (200, `LensArn` + `Status`), the identical retry under token T that spells the same lens the other way — the ARN with a version pinned after it, which `src/state.rs:4082-4093` says names that same lens — hashes to a different fingerprint, so the ledger lookup takes the `token.fingerprint != fingerprint` branch and answers 409 `ConflictException` ("A request with ClientRequestToken T was already submitted with different parameters.") instead of the replayed success the token exists to give. The retry is the same request by the crate's own lens-identity rule, and a success is turned into an error. The token really does reach that branch: it is remembered unconditionally at the end of the handler and nothing in `remove_lens`-adjacent cleanup drops it for a lens that still exists.

The crate already owns the normalisation this path skips (`crate::state::lens_alias`), and the two sibling idempotent lens-keyed upgrades call it INSIDE the fingerprint for exactly this reason, with the rule spelled out in a comment. Note this is a DIFFERENT operation from the already-filed DeleteLensShare / DeleteLens / CreateLensShare instances of the same mistake; none of those issues mentions `import_lens.rs`, whose outcome diverges observably in the same way (409 instead of the original 200). The fix must keep ImportLens's create-vs-update distinction (a NON-ARN `LensAlias` deliberately means "no ARN specified", `src/ops/import_lens.rs:352-357`, pinned by `tests/all/custom_lens_import.rs:648-650`), so only ARN spellings that resolve to one stored lens may fingerprint alike.

## Evidence

Model rule the handler is implementing — `ImportLensInput$ClientRequestToken` (`@idempotencyToken`), as the handler quotes it, `src/ops/import_lens.rs:211-212`: "`@idempotencyToken` replay: the same token with the same parameters reports the original import instead of importing a second time."

Side 1 — the fingerprint is over the RAW spelling: `src/ops/import_lens.rs:190-194` — `let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({ "JSONString": &input.json_string, "LensAlias": &input.lens_alias, "Tags": &tags, }));` and the refusal it drives, `src/ops/import_lens.rs:213-223` — `if let Some(token) = store.import_lens_tokens.get(&input.client_request_token) { let lens_id = token.lens_id.clone(); if token.fingerprint != fingerprint { return Err(ImportLensErrorModeled::ConflictException(crate::state::idempotency_token_conflict(crate::state::LENS_RESOURCE_TYPE, lens_id, &input.client_request_token,)).into()); }`.

Side 1b — yet the lens itself is resolved spelling-insensitively, 30 lines later: `src/ops/import_lens.rs:247` — `let record = store.lens_by_alias_mut(alias).ok_or_else(|| { ... lens_not_found(alias) ... })?;`, over the identity rule in `src/state.rs:4082-4093` — "Whether `alias` names `record`: an ARN must agree with the lens's own ARN (so the account and Region it carries have to match), optionally pinning a version after it; a bare id names the lens whose id it is." with `alias == record.lens_arn || alias.strip_prefix(record.lens_arn.as_str()).is_some_and(|version| version.starts_with('/'))`.

Side 2 — the sibling that applies the rule, with the rule stated: `src/ops/upgrade_lens_review.rs:212-224` — "The parameters besides the token are the three this request carries, with the lens taken as the review it names rather than as the spelling it was named by — a lens review is one review however the request spells its lens (`crate::state::lens_alias`), so a retry that names it the other way is the same request." followed by `"LensAlias": crate::state::lens_alias(&input.lens_alias),` (`:221`). Identical in `src/ops/upgrade_review_template_lens_review.rs:210`.

Test coverage that stays unchanged: the idempotency table `tests/all/custom_lens_import.rs:761-832` (`an_import_is_idempotent_per_client_request_token`) varies only the DOCUMENT and the TAGS of the second request — every row passes `None` for the alias (`import(&client, document, None, "tok-idempotent", tags)`, `:783`), i.e. the create path — so normalising the ARN spelling inside the fingerprint leaves all three rows green, as do the official-lens / unknown-ARN rows at `:603-747`, which are refused before the ledger matters.

## Steps to Reproduce

1. ImportLens with no LensAlias to mint a custom lens; note the returned LensArn (`arn:aws:wellarchitected:<region>:<account>:lens/<lensId>`) and wait for the import to land
2. CreateLensVersion LensVersion="1" on that ARN so the lens carries a published version
3. Re-import over that lens: PUT /importLens with ClientRequestToken=tok-1, LensAlias=<the lens ARN>, JSONString=<a new document> — answers 200 with LensArn and Status IN_PROGRESS
4. Retry the identical request under the same token, spelling the lens the other way that src/state.rs:4082-4093 says names the same lens: LensAlias=<the lens ARN>/1 (the version-pinned form)
5. Observe 409 ConflictException ("A request with ClientRequestToken tok-1 was already submitted with different parameters.") from the fingerprint mismatch at src/ops/import_lens.rs:213-223, instead of the replayed 200 the @idempotencyToken rule quoted in that handler's own comment (:211-212) promises
6. Compare with src/ops/upgrade_lens_review.rs:220-224, which passes the alias through crate::state::lens_alias before fingerprinting so the equivalent retry replays
