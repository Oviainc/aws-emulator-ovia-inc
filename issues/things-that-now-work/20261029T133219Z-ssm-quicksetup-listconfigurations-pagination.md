Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 07e288609021e6571eb22a2173d293968d6da2ab

# A StartingToken minted by another Region's endpoint is accepted and silently drops the requesting Region's own configurations

_Filed: 2026-10-29T13:32:19Z_
_Operation: ListConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `07e288609021e6571eb22a2173d293968d6da2ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConfigurations` resumes from a page token that carries only the listing kind and the store key of the configuration the next page starts at. The token is NOT scoped to the Region whose endpoint issued it, and `configurations` is one store-wide `BTreeMap` keyed by a service-generated uuid that carries no Region at all. So a token minted for a request signed for Region A is accepted on a request signed for Region B and used as the start of the shared range: every configuration of Region B whose id sorts BEFORE that foreign key is skipped, and since a listing only ever walks forward from the token, no later page ever reaches them. The caller is handed a short (often empty) page together with the empty `NextToken` that means "there is no next page", with no error at all.

This contradicts the operation's own modelled sentence — it must return the configurations in the REQUESTING Region — and the handler's own stated contract, and it defeats the rule `paginate` documents for itself ("paging still visits every record exactly once"). The handler already refuses a token it did not issue (a token tagged for the other listing), so the mechanism to refuse exists; the token simply does not carry the issuing Region for the check to use.

Note this is the ListConfigurations half of the defect: the already-open ListConfigurationManagers/pagination issue (20261029T102902Z) covers the manager listing only, and this operation's token (a bare configuration uuid, not an ARN that embeds its Region) is a distinct code path with a distinct token kind.

## Evidence

Modelled contract — `smithy_query(query="doc", shape="ListConfigurations")`:
"Returns configurations deployed by Quick Setup in the requesting Amazon Web Services account and Amazon Web Services Region."

The token carries no Region — `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/state.rs:1671`:
```
pub fn encode_page_token(kind: &str, key: &str) -> String {
    ::hex::encode(format!("{kind}\u{1f}{key}"))
```
and the decode checks only the kind tag — `state.rs:1677`:
```
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    ...
    decoded.strip_prefix(&format!("{kind}\u{1f}")).map(str::to_string)
```

The handler resumes from that key with no Region check — `src/ops/list_configurations.rs:182-198` then `:219-222`:
```
        let start = match crate::state::resume_token(input.starting_token.as_deref()) {
            Some(token) => Some(
                crate::state::decode_page_token(
                    crate::state::CONFIGURATION_PAGE_TOKEN_KIND,
                    token,
                )
...
        let (page, next_token) = crate::state::paginate(
            crate::state::CONFIGURATION_PAGE_TOKEN_KIND,
            store.configurations.range(start.unwrap_or_default()..),
```
while the same handler states the contract it breaks — `src/ops/list_configurations.rs:158-160`:
```
        // Quick Setup is a regional control plane, so this listing reports the
        // configurations of the managers created through the endpoint of the Region this
        // request was signed for and no others.
```
and `:178-181`:
```
        // A token this operation did not issue is refused rather than silently read as
        // "start from the beginning", which would re-serve a page the caller already had.
```

The store key the token names carries no Region — `state.rs:1198-1200`:
```
    /// Every configuration, keyed by its service-generated id, for the same reason —
    /// and reached, for the same reason, through [`Self::configuration_in`].
    pub configurations: BTreeMap<String, ConfigurationRecord>,
```
(ids come from `fresh_uuid`, `state.rs:1226-1238`, whose leading groups are clock-mixed, so a Region's ids are interleaved with every other Region's.)

The invariant `paginate` claims — `state.rs:1609-1610`:
```
/// Either way the token names where to resume,
/// so paging still visits every record exactly once.
```

Test coverage: `tests/all/pagination_tokens.rs:33-34` signs every request for ONE region (`const REGION: &str = "us-west-2";`) and never replays a token under another credential scope; `tests/all/region_scoping.rs:9-17` asserts only the TOKENLESS cross-Region case ("A request signed for any other Region lists none of it (its page is empty and its `NextToken` is the empty string ...)"). Nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the emulator with the ssm-quicksetup service mounted.
2. Sign two CreateConfigurationManager calls for us-west-2 (POST /configurationManager, each definition naming TargetRegions and TargetOrganizationalUnits), so us-west-2 holds two configurations.
3. Sign POST /listConfigurations for us-west-2 with {"MaxItems": 1} and keep the non-empty NextToken T (it hex-decodes to `configuration\u{1f}<configuration uuid>`).
4. Sign two CreateConfigurationManager calls for us-east-1 the same way, and confirm POST /listConfigurations for us-east-1 with {} reports both of that Region's configurations.
5. Now sign POST /listConfigurations for us-east-1 with {"StartingToken": T}.
6. Observed: 200 with a ConfigurationsList that omits every us-east-1 configuration whose service-generated id sorts before the id inside T (with ids interleaved across Regions this drops one or both), plus NextToken "" — i.e. "there is nothing more in this Region". Expected: the token is not one this endpoint issued, so it is refused the way a foreign-kind token already is (400 ValidationException), rather than answering a page that silently omits the requesting Region's own configurations.
