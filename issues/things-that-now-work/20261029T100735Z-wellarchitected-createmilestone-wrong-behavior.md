Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f836000fedddc1a4148e8a84fc8c0ec776ea3f4e

# CreateMilestone's idempotency fingerprint is spelling-sensitive, so a retry naming the same milestone with an equivalent spelling is refused 409 instead of replaying the milestone

_Filed: 2026-10-29T10:07:35Z_
_Operation: CreateMilestone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f836000fedddc1a4148e8a84fc8c0ec776ea3f4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateMilestone decides what makes two milestone names THE SAME NAME through the crate's one loose-name rule (`crate::state::normalize_unique_name`, which drops capitalization and whitespace): `record_milestone` refuses a second milestone whose name differs from a stored one only in case or whitespace, and the refusal it builds even tells the caller so ("Capitalization and whitespace are ignored in milestone names").

But the handler builds its `@idempotencyToken` replay fingerprint over the RAW `input.milestone_name` string. Consequence: after a create under token T that named the milestone "Release Candidate" has completed successfully (200 carrying the minted `MilestoneNumber`), the identical retry under token T spelling it "release candidate" — which the crate's own rule says is the SAME name, so the same milestone of the same workload — hashes to a different fingerprint, takes the mismatch branch, and answers 409 `ConflictException` with the message "A request with ClientRequestToken <T> was already submitted with different parameters." instead of the replayed 200 carrying the original `MilestoneNumber` that the token exists to produce. A success is turned into an error for a request the crate itself treats as naming one milestone, and the refusal's own wording ("different parameters") contradicts the identity rule the same crate applies one call later.

The crate already owns the normalisation this path skips, and its idempotent siblings apply exactly this treatment to their own identity-bearing parameter inside the fingerprint: `UpgradeLensReview` and `UpgradeReviewTemplateLensReview` pass the lens through `crate::state::lens_alias` before fingerprinting and state the reason — "a retry that names it the other way is the same request". The same raw `MilestoneName` sits in the two upgrade fingerprints as well (`upgrade_lens_review.rs:220-224`, `upgrade_profile_version.rs:198-202`), which record a milestone through the very same `record_milestone`, so one fix covers all three. Normalising the fingerprint changes nothing for any retry that spells the name consistently, so every existing row stays green.

## Evidence

Model rule the handler implements — `CreateMilestoneInput$ClientRequestToken`, quoted in the handler's own header at `src/ops/create_milestone.rs:53-61`: "A unique case-sensitive string used to ensure that this request is idempotent (executes only once). ... If you retry a request with the same client request token and the same parameters after the original request has completed successfully, the result of the original request is returned."

Side 1 — the fingerprint is over the raw spelling, `src/ops/create_milestone.rs:171-174`:
```
        let fingerprint = crate::state::parameter_fingerprint(&serde_json::json!({
            "MilestoneName": &input.milestone_name,
            "WorkloadId": &input.workload_id,
        }));
```
and the two branches it feeds — the mismatch refusal `src/ops/create_milestone.rs:180-194` (`if token.fingerprint != fingerprint { ... return Err(CreateMilestoneErrorModeled::ConflictException(crate::state::idempotency_token_conflict(...)).into()); }`), whose sentence is `src/state.rs:9609-9612` — "A request with ClientRequestToken {client_request_token} was already submitted with different parameters." — versus the replay `src/ops/create_milestone.rs:195-200` (`if let Some(recorded) = store.milestone(&token.workload_id, token.milestone_number) { return Ok(CreateMilestoneOutput { workload_id: ..., milestone_number: Some(recorded.milestone_number) }); }`).

Side 2 — the crate's own rule that those two spellings are ONE name, `src/state.rs:8090-8103`: "The form a name whose uniqueness this service decides LOOSELY is compared in: capitalization and whitespace are dropped, so two spellings that differ only in either are the same name. ... it is how every name this service meters for uniqueness is compared, so the rule cannot differ between the resources that carry it: a workload name ... and a milestone name within its workload" followed by `pub fn normalize_unique_name(name: &str) -> String { name.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect() }`. It is applied to the milestone name on both sides at `src/state.rs:7572-7575` — `let normalized = normalize_unique_name(name); self.milestones.get(workload_id)?.values().find(|record| normalize_unique_name(&record.milestone_name) == normalized)` — reached from `record_milestone` at `src/state.rs:7615`, and stated to the caller in the refusal at `src/state.rs:9592-9595`: "Milestone with name {existing_name} already exists under workload {workload_id}. Capitalization and whitespace are ignored in milestone names."

Side 2b — the sibling idempotent handlers that DO normalise the identity-bearing parameter inside the fingerprint, with the rule spelled out, `src/ops/upgrade_lens_review.rs:212-224`: "The parameters besides the token are the three this request carries, with the lens taken as the review it names rather than as the spelling it was named by — a lens review is one review however the request spells its lens (`crate::state::lens_alias`), so a retry that names it the other way is the same request." followed by `"LensAlias": crate::state::lens_alias(&input.lens_alias),` (same in `src/ops/upgrade_review_template_lens_review.rs:204-212`).

Test coverage that stays unchanged: `tests/all/milestone_name_uniqueness.rs:140-162` walks the respellings ("identical", "upper", "lower", "padded", "whitespace-removed", "tab-separated") but gives each row its OWN token (`&format!("tok-dup-{index}-{spelling}")`), so every row still reaches the name-conflict path and still asserts `assert_name_conflict`; no row sends a respelled name under an ALREADY-SPENT token, so nothing pins the behaviour reported here.

## Steps to Reproduce

1. CreateWorkload to obtain a WorkloadId
2. POST /workloads/<WorkloadId>/milestones with {"MilestoneName": "Release Candidate", "ClientRequestToken": "tok-1"} — 200, MilestoneNumber 1
3. Retry the same request under the SAME token with the name spelled the way the crate's own rule (src/state.rs:8090-8103) says is the same name: {"MilestoneName": "release candidate", "ClientRequestToken": "tok-1"}
4. Observe 409 ConflictException "A request with ClientRequestToken tok-1 was already submitted with different parameters." from the mismatch branch at src/ops/create_milestone.rs:180-194, instead of the replayed 200 carrying MilestoneNumber 1 promised by the ClientRequestToken doc quoted at src/ops/create_milestone.rs:53-61
5. Compare with src/ops/upgrade_lens_review.rs:220-224, which passes its identity-bearing parameter through the crate's normalisation before fingerprinting so the equivalent retry replays; ListMilestones confirms only the one milestone exists either way
