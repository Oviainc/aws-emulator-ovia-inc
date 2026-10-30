Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# Replace the private `unanchored` / `enclosing_collection` copies in the refusal formatter with the `service_common::validation` helpers the crate already depends on

_Filed: 2026-10-28T23:36:55Z_
_Operation: DeregisterSubscriptionProvider | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service/validation.rs` is the single formatter every operation's boundary refusal is worded by (`service.rs:110` -> `validation::refused`). Two of the rules it implements are not this service's dialect at all — stripping the `@pattern` anchors, and recognising a collection-element path so the clause is reported at the collection member — and the module's own MIGRATION NOTE says they are duplicates of `services/common/src/validation.rs`, which this crate ALREADY depends on (`register_subscription_provider.rs:490` calls `::service_common::jwt::read`).

So the same logic lives twice in reachable code: the private `unanchored` here versus `service_common::validation::unanchored`, and the private `enclosing_collection` here versus `service_common::validation::list_of_element`. The two copies can drift — and in one corner they ALREADY disagree: on a pattern whose trailing `$` is escaped, this crate's copy returns `body` (the expression with its leading `^` already stripped) while the shared copy returns the whole `regex` unchanged, so the two would quote different expressions for such a model text. Neither of this service's two `@pattern`s (`SubscriptionProviderArn`, `SecretArn`) ends in an escaped `$`, so switching to the shared helpers leaves every response and every existing test byte-identical.

Scope: delete the two private helpers and call the `service_common::validation` ones; the tag-API taxonomy and the collection-value rendering stay here, exactly as the note says they should. No response text and no test expectation changes.

## Evidence

The crate's own admission, `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/service/validation.rs:57-63`:

    //! MIGRATION NOTE (`services/common/src/validation.rs`): the three rules that are NOT a
    //! per-service dialect — the requirement half read off the framework's own wording, the
    //! subject half, and a `@pattern` printed without its anchors — are staged in that
    //! module and inlined here as they are in ~24 other service crates, because moving them
    //! is a change of its own rather than a side effect of this service's issue. This copy
    //! is one of the ones that migration has to collect; what is genuinely this service's
    //! own and stays here is the tag-API taxonomy and the collection-value rendering.

Copy 1 — `src/service/validation.rs:140-153`:

    fn unanchored(pattern: &str) -> &str {
        let body = pattern.strip_prefix('^').unwrap_or(pattern);
        match body.strip_suffix('$') {
            Some(trimmed) => {
                let escapes = trimmed.len() - trimmed.trim_end_matches('\\').len();
                if escapes % 2 == 0 { trimmed } else { body }
            }
            None => body,
        }
    }

The crate API that replaces it — `services/common/src/validation.rs:673-682`:

    pub fn unanchored(regex: &str) -> String {
        let Some(body) = regex.strip_prefix('^') else {
            return regex.to_string();
        };
        match body.strip_suffix('$') {
            Some(inner) if !ends_with_escape(inner) => inner.to_string(),
            _ => regex.to_string(),
        }
    }

Copy 2 — `src/service/validation.rs:162-168`:

    fn enclosing_collection(path: &str) -> Option<&str> {
        let (head, position) = path.strip_suffix(COLLECTION_STEP)?.rsplit_once('.')?;
        if position.is_empty() || !position.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(head)
    }

The crate API that replaces it — `services/common/src/validation.rs:764-770`:

    pub fn list_of_element(leaf_path: &str) -> Option<&str> {
        let head = leaf_path.strip_suffix(".member")?;
        let (list, position) = head.rsplit_once('.')?;
        let is_position = !position.is_empty() && position.bytes().all(|byte| byte.is_ascii_digit());
        (is_position && !list.is_empty()).then_some(list)
    }

Proof `service_common` is already in this crate's dependency graph — `src/ops/register_subscription_provider.rs:490`:

    let token = match ::service_common::jwt::read(credential) {

Existing tests that pin the output of both spans, and must stay green unchanged:
* `tests/all/error_taxonomy.rs:252` `a_pattern_violation_quotes_the_expression_without_its_anchors` — asserts the exact sentence `"1 validation error detected: Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy regular expression pattern: {pattern}"` for `GetRegisteredSubscriptionProvider`, `DeregisterSubscriptionProvider` and `RegisterSubscriptionProvider`, plus `!quoted.starts_with('^') && !quoted.ends_with('$')` (error_taxonomy.rs:329-332) — this is `unanchored`.
* `tests/all/error_taxonomy.rs:339` `an_enum_violation_in_a_list_is_reported_at_the_list_member` — rows `[NotARealProvider]`, `[SUSE]`, `[redhat]`, `[SUSE, Oracle]` reported at the list member — this is `enclosing_collection`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/service/validation.rs` and read the MIGRATION NOTE at lines 57-63, which names `services/common/src/validation.rs` as the module the non-dialect rules are staged in.
2. Compare the private `fn unanchored` (validation.rs:140-153) with `pub fn unanchored` (services/common/src/validation.rs:673-682): the same anchor-strip rule, written twice.
3. Compare the private `fn enclosing_collection` (validation.rs:162-168) with `pub fn list_of_element` (services/common/src/validation.rs:764-770): the same `<collection>.<position>.member` recognition, written twice.
4. Confirm `service_common` is already a dependency of this crate: `src/ops/register_subscription_provider.rs:490` calls `::service_common::jwt::read`.
5. Note the one place the copies already disagree: for a pattern ending in an escaped `$`, the local copy returns `body` (leading `^` already stripped) while the shared copy returns `regex` unchanged — a drift that would quote two different expressions.
6. Run `cargo test -p service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions` and observe `a_pattern_violation_quotes_the_expression_without_its_anchors` and `an_enum_violation_in_a_list_is_reported_at_the_list_member` pass; they must still pass byte-for-byte after the two private helpers are replaced by the shared ones.
