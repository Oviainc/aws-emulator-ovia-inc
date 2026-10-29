Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 356b06cff97ee4bf623fbe99508d16145f9b3a96

# A DeregisterSubscriptionProvider between two pages shifts the offset cursor, so the provider walk silently omits a registration

_Filed: 2026-10-28T21:10:22Z_
_Operation: ListRegisteredSubscriptionProviders | Class: pagination_

> _Note: this issue was identified against the code base at commit `356b06cff97ee4bf623fbe99508d16145f9b3a96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListRegisteredSubscriptionProviders` pages with `crate::state::paginate`, whose cursor is a plain OFFSET into the list rebuilt from `provider_order` on every page (`providers_in_order()`). `DeregisterSubscriptionProvider` removes the ARN from that same `provider_order` vector (`remove_provider`), which shifts every later element one position DOWN. A continuation page therefore resolves its stored offset against a shorter sequence and skips the registration(s) that moved across the cursor — and because the shortened list makes `end == items.len()`, the response carries NO `NextToken`, so the walk terminates as a successful, complete-looking listing that never reported a registration the store still holds.

The crate itself states this exact hazard and defends the SIBLING walks against it: `src/state/discovery.rs:155-159` says a continuation page runs no discovery pass precisely because "a refresh between two pages of one walk could move a record across the cursor and make the walk skip or repeat it". The provider walk is protected against nothing: the removal that moves a record across its cursor is a plain caller-driven `DeregisterSubscriptionProvider`, and the store comment at `src/state.rs:306` claims the opposite property ("so a list is stable and pageable").

Concretely, with three registrations [P1,P2,P3] and `MaxResults: 2`: page 1 reports [P1,P2] and mints offset 2; a deregistration of P1 leaves [P2,P3]; page 2 resolves offset 2 against a 2-element list, returns an EMPTY `RegisteredSubscriptionProviders` with no `NextToken`, and P3 — still registered, still readable through `GetRegisteredSubscriptionProvider` — was never reported by the walk.

This is a different operation and a different mechanism from the already-open discovery-list issue (`...listlinuxsubscriptions-pagination`, which is about the sibling list's discovery REFRESH mutating the shared instance store); no issue covers the provider listing.

## Evidence

The hazard the crate states and defends the discovery walks against — `src/state/discovery.rs:155-159`:
```
/// `next_token` is the cursor the reporting request carried. A request CONTINUING a walk
/// carries the cursor the previous page minted, which is a position in the list THAT page
/// reported, so no pass runs for it: a refresh between two pages of one walk could move a
/// record across the cursor and make the walk skip or repeat it. A walk therefore reports
/// the inventory of the pass its first page ran.
```

The provider list paging on a bare offset — `src/ops/list_registered_subscription_providers.rs:166-183`:
```
        let selected: Vec<crate::state::SubscriptionProviderRecord> = state
            .providers_in_order()
            .into_iter()
            .filter(|record| sources.is_empty() || sources.contains(&record.source))
            .collect();

        let (page, next_token) = crate::state::paginate(
            &selected,
...
            input.max_results,
            input.next_token.as_deref(),
        )
```

The cursor is a position re-resolved against the rebuilt sequence, and a shortened sequence ends the walk without a token — `src/state.rs:842-866`:
```
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
    if offset > items.len() {
        return Err(StateError::Validation(
            "NextToken does not refer to a position in this list".to_string(),
        ));
    }
...
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
```

The write that moves records across that offset — `src/state.rs:555-560`, reached from `src/ops/deregister_subscription_provider.rs:157` (`state.remove_provider(&input.subscription_provider_arn);`):
```
    /// Remove a registration from the store and from the order that reaches it.
    pub fn remove_provider(&mut self, arn: &str) -> Option<SubscriptionProviderRecord> {
        let record = self.providers.remove(arn)?;
        self.provider_order.retain(|stored| stored != arn);
        Some(record)
    }
```
against the stability the store claims for that very vector — `src/state.rs:306-307`:
```
    /// Provider ARNs in registration order, so a list is stable and pageable.
    pub provider_order: Vec<String>,
```

Test coverage: `tests/all/subscription_providers.rs:135-192` (`the_list_pages_exactly_once_over_every_registration`) walks every page size over an UNCHANGING store and asserts "paging must cover every registration exactly once, in order"; `tests/all/subscription_providers.rs:218-248` (`deregistration_is_observable_by_every_later_caller`) deregisters and then re-lists with `json!({})` — an unpaged list. Neither test deregisters INSIDE a paged walk, so the gap is untested.

## Steps to Reproduce

1. Register (or seed) three subscription providers P1, P2, P3 so the store holds them in registration order.
2. POST /subscription/ListRegisteredSubscriptionProviders with {"MaxResults": 2} — the response reports P1 and P2 and a NextToken (offset 2).
3. Before sending page 2, POST /subscription/DeregisterSubscriptionProvider with P1's SubscriptionProviderArn (200, and `remove_provider` drops P1 from `provider_order`).
4. POST /subscription/ListRegisteredSubscriptionProviders with {"MaxResults": 2, "NextToken": <token from step 2>}: the offset 2 is resolved against the now 2-element sequence, so the page is EMPTY and carries no NextToken — the walk ends.
5. POST /subscription/GetRegisteredSubscriptionProvider with P3's ARN: 200, the registration is still there — it was silently omitted from a walk that reported success, the skip/repeat outcome src/state/discovery.rs:155-159 says a cursor must be protected from.
