Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# De-duplicate the "Tags go into the idempotency fingerprint sorted" computation shared by the two create fingerprints

_Filed: 2026-10-25T00:42:12Z_
_Operation: CreateApplication | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a tag set carries no order the caller chose, so the pairs go into the `ClientToken` fingerprint SORTED, key then value, each as its own part" is implemented twice in this crate's state module, as the same nine lines: once in `application_fingerprint` (used by `CreateApplication`) and once in `trusted_token_issuer_fingerprint` (used by `CreateTrustedTokenIssuer`). The two copies can drift — a change to how tag pairs are folded in (normalisation, de-duplication, a different ordering) applied to one fingerprint and not the other would silently make one of the two creates judge a replay differently from the other, even though both implement the same `@idempotencyToken` sentence.

The fix is a single private helper in `state.rs` that pushes the sorted tag parts onto the `parts` vector, called from both fingerprint functions. Both fingerprints keep producing the same digests for the same inputs, so every response and every existing test stays unchanged.

## Evidence

Copy 1 — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:2716-2724` in `application_fingerprint` (whose doc already names the shared rule: "`Tags` go in SORTED, because a tag set carries no order the caller chose; everything else is length-prefixed in a fixed order by [`request_fingerprint`]", `state.rs:2690-2691`):

```rust
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
    request_fingerprint(&parts)
```

Copy 2 — `src/state.rs:2755-2764` in `trusted_token_issuer_fingerprint`, byte-for-byte the same block (its doc restates the same rule at `state.rs:2733`: "`Tags` go in SORTED, because a tag set carries no order the caller chose."):

```rust
    let mut sorted: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    sorted.sort_unstable();
    for (key, value) in sorted {
        parts.push(key.to_string());
        parts.push(value.to_string());
    }
    request_fingerprint(&parts)
```

Both copies are exercised end to end today, so a refactor is covered: `tests/all/application_assignments.rs:499-502` (`a_replayed_client_token_mints_no_second_application_and_a_changed_parameter_conflicts`, which varies `Tags` across replays — see the table at `application_assignments.rs:164`) and `tests/all/trusted_token_issuers.rs:306-309` (`a_replayed_client_token_mints_no_second_issuer_and_a_changed_parameter_conflicts`, with the tag-count axis described at `trusted_token_issuers.rs:121`).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs` and compare `application_fingerprint` (lines 2692-2726) with `trusted_token_issuer_fingerprint` (lines 2734-2765).
2. Observe that the tag-folding block (sort the (key, value) pairs, then push key and value as separate parts, then `request_fingerprint`) is identical in both — one rule, two copies.
3. Confirm both are reachable: `CreateApplication` calls the first (`src/ops/create_application.rs:317-325`), `CreateTrustedTokenIssuer` the second (`src/ops/create_trusted_token_issuer.rs:224-230`).
4. Extract the block into one helper called by both, and re-run `tests/all/application_assignments.rs` and `tests/all/trusted_token_issuers.rs`: the same digests, the same 200/ConflictException answers.
