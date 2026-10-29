Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0b62ce506db227f8753aee084e9b25daeb773a55

# The ARN-path Domain cross-check is implemented twice in state.rs (resolve_product_user and resolve_instance_user)

_Filed: 2026-10-28T16:07:06Z_
_Operation: StopProductSubscription | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0b62ce506db227f8753aee084e9b25daeb773a55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a record addressed by ARN is only the one the caller asked for when the `Domain` the request names is the stored record's" is written out twice in `src/state.rs`, once in `resolve_product_user` (the resolution `StopProductSubscription` runs) and once in `resolve_instance_user` (the one `DisassociateUser` runs). Both copies compute the identical predicate over the identical normaliser — `let named = domain_key(domain); if named.is_some() && named != domain_key(record.domain.as_deref())` — and carry the same comment with one word changed ("a subscription" / "an association"). Only the action taken on a mismatch differs (one returns `Err(StateError::NotFound(..))`, the other `None`, because this surface fails closed).

Two copies of one comparison can drift: a change to how `Domain` is matched (or to `domain_key`'s normalisation of the empty string) has to be made in both places, and nothing in the file ties them together — even though every other shared identity rule in this module is deliberately written once and named as such (`product_user_key`/`instance_user_key`: "The ONE place that key is built", `domain_key`: "The `Domain` half of a user's identity, normalised").

The fix is to extract the predicate (the comparison plus its one comment) into a single helper beside `domain_key`, and have both resolutions call it, each keeping its own refusal. Every response and every existing test stays exactly as it is.

## Evidence

Copy 1 — `src/state.rs:1134-1140` (`resolve_instance_user`):
```
        // `Domain` is part of which user a request names, so an association addressed by
        // ARN is also only the one the caller asked for when the domain the request names
        // is the stored user's. The identity lookup above already matched on it.
        let named = domain_key(domain);
        if named.is_some() && named != domain_key(record.domain.as_deref()) {
            return None;
        }
```
Copy 2 — `src/state.rs:2029-2039` (`resolve_product_user`):
```
        // `Domain` is part of which user a request names, so a subscription addressed by
        // ARN is also only the one the caller asked for when the domain the request names
        // is the stored user's. The identity lookup above already matched on it.
        let named = domain_key(domain);
        if named.is_some() && named != domain_key(record.domain.as_deref()) {
            return Err(StateError::NotFound(no_such_subscription(
                &record.username,
                &record.product,
                domain,
            )));
        }
```
The module's own standard for a shared identity rule, which these two copies depart from — `src/state.rs:2311-2315` (`product_user_key`):
```
/// The identity of the product subscription a request addresses, or a record holds.
///
/// The ONE place that key is built, so the occupancy a `StartProductSubscription` refuses
/// a duplicate on and the lookup a `StopProductSubscription` resolves one by cannot
/// disagree about which user is which.
```
and `src/state.rs:2297-2304` (`domain_key`), the normaliser both copies call.

Test coverage that must stay unchanged: `tests/all/user_domain_names_the_user.rs` pins both planes' `Domain` matching — `a_domain_names_which_user_an_association_belongs_to` (line 348) for the association plane and the product-subscription rows above it (lines 330-340) for the subscription plane.

## Steps to Reproduce

1. Open services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/state.rs and read lines 1134-1140 (resolve_instance_user).
2. Read lines 2029-2039 (resolve_product_user) and compare: the same `domain_key`-normalised comparison and the same comment appear a second time, with only the refusal differing.
3. Note that neither call site references the other, and that `domain_key` (line 2305) is the only part of the rule that is shared, so a change to the matching rule has to be applied twice.
