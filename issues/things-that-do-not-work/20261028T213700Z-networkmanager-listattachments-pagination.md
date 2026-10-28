Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 772fb5ab5b7496e2df886721dbd75d9882bc460e

# A NextToken is not scoped to the listing that minted it, so a token from another listing is accepted and silently answered with an empty page

_Filed: 2026-10-28T21:37:00Z_
_Operation: ListAttachments | Class: pagination_

> _Note: this issue was identified against the code base at commit `772fb5ab5b7496e2df886721dbd75d9882bc460e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated listing of this crate refuses a `NextToken` it cannot decode, and each states why: "A token this service did not mint names no position in the listing, so it is refused on the member rather than answered with an empty page." But `decode_page_token` only checks ONE service-wide prefix (`networkmanager:`) and hands back the raw key — it records nothing about WHICH listing minted the token. A token minted by any of the 24 listings is therefore accepted by all of them.

`paginate` then resolves that foreign key against a completely different key space. When no stored key sorts after it, `position(...)` answers `None`, `start` becomes `entries.len()`, `end == entries.len()`, and the handler answers HTTP 200 with an EMPTY list and NO `NextToken` — indistinguishable from "this account holds no attachments at all" — even though the store holds attachments. That is precisely the "answered with an empty page" outcome the handler's own comment says must not happen for a token that names no position in the listing.

The reverse mix-up is equally silent: when the foreign key sorts before every stored key, `start` is 0 and the listing replays its FIRST page, so a client that carries a token across operations loops over the first page instead of being told the token is wrong.

## Evidence

The rule the handler states — src/ops/list_attachments.rs:161-163:
```
        // A token this service did not mint names no position in the listing, so it is
        // refused on the member rather than answered with an empty page.
        let after = match input.next_token.as_deref() {
```
The decoder, which knows only one service-wide prefix — src/state.rs:8343-8362:
```
/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";
...
fn encode_page_token(last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}
...
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}
```
What a foreign key does to the page — src/state.rs:8374-8394:
```
    let start = match after {
        Some(key) => entries
            .iter()
            .position(|(candidate, _)| candidate.as_str() > key)
            .unwrap_or(entries.len()),
        None => 0,
    };
...
    let end = start.saturating_add(limit).min(entries.len());
    let next_token = if end < entries.len() { ... } else { None };
```
The key spaces that collide: attachments are keyed by attachment id (`src/ops/list_attachments.rs:183-185`: "The attachments are held in a `BTreeMap` keyed by attachment id, so the listing order is stable and the page token below can name a key offset into it"), sites by site id (`src/ops/get_sites.rs:162`), policy versions by a zero-padded integer (`src/state.rs:1073-1075`, `policy_version_page_key` → `format!("{version_id:010}")`), change events by `IdentifierPath` (`src/ops/get_core_network_change_events.rs:211-213`). `"attachment-…" < "site-…"`, so a GetSites token lands past the end of the attachment listing; `"attachment-…" > "0000000001"`, so a ListAttachments token lands past the end of the policy-version listing.

All 24 listings share the one decoder (grep `decode_page_token`: describe_global_networks, get_connections, get_connect_peer_associations, get_core_network_change_events, get_core_network_change_set, get_customer_gateway_associations, get_devices, get_link_associations, get_links, get_network_resource_counts, get_network_resource_relationships, get_network_resources, get_network_telemetry, get_sites, get_transit_gateway_connect_peer_associations, get_transit_gateway_registrations, list_attachment_routing_policy_associations, list_attachments, list_connect_peers, list_core_network_policy_versions, list_core_network_prefix_list_associations, list_core_network_routing_information, list_organization_service_access_status).

Test coverage: the paging tests only ever feed a token back to the listing that issued it (tests/all/get_customer_gateway_associations_paging.rs, tests/all/get_transit_gateway_registrations_paging.rs, tests/all/get_transit_gateway_connect_peer_associations_paging.rs, tests/all/list_attachments.rs), so scoping the token leaves every existing assertion unchanged.

## Steps to Reproduce

1. CreateGlobalNetwork, CreateSite twice on it, then CreateCoreNetwork with a policy and CreateVpcAttachment so at least one attachment is stored
2. GET /global-networks/{globalNetworkId}/sites?maxResults=1 → 200 with a NextToken (hex of "networkmanager:site-0123456789abcdef0")
3. GET /attachments?nextToken=<that token> → 200 with "Attachments": [] and no NextToken, although the attachment from step 1 exists: src/state.rs:8359 accepts the token because it carries the service-wide prefix, and src/state.rs:8377 finds no attachment key greater than "site-…", so start == entries.len()
4. Expected from the handler's own rule (src/ops/list_attachments.rs:161-162): the token names no position in THIS listing, so it is refused on the NextToken member — not answered with an empty page
5. Reverse direction: GET /attachments?maxResults=1 → NextToken; GET /global-networks/{globalNetworkId}/sites?nextToken=<that> → 200 with the FIRST page of sites again, so a client following the wrong token never advances
