Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 032f9a5bcc1a20e2d342a3403d3d533abd316530

# A StartingToken issued by another Region's endpoint is accepted and silently skips the requesting Region's own managers

_Filed: 2026-10-29T10:29:02Z_
_Operation: ListConfigurationManagers | Class: pagination_

> _Note: this issue was identified against the code base at commit `032f9a5bcc1a20e2d342a3403d3d533abd316530`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The page token this service issues carries only the listing kind and the store key of the record the next page starts at — it is NOT scoped to the Region whose endpoint issued it. `ListConfigurationManagers` therefore accepts a token minted for a request signed for Region A when it is replayed on a request signed for Region B, and uses the ARN inside it as the start of the shared, cross-Region `managers` range. Every manager of Region B whose ARN sorts BEFORE that foreign ARN is skipped, and because a listing only ever walks forward from the token, no later page ever visits them: the caller is handed a page (often an empty one, with the empty `NextToken` that means "no next page") that under-reports what its own Region holds, with no error at all.

This is deterministic, not theoretical: a manager ARN embeds the Region it was created in (`crate::state::manager_arn`), so a token naming a `us-west-2` manager sorts after every `us-east-1` manager. Replaying that token on a `us-east-1`-signed request answers an empty `ConfigurationManagersList` and an empty `NextToken`, i.e. "this account holds no configuration managers in us-east-1", while `us-east-1` managers are stored and a tokenless listing reports them.

Every other path that resolves a caller-supplied identifier applies this crate's centralised Region scoping, and the handler itself already refuses a token it did not issue rather than silently misreading it — the token simply does not carry the one thing (the issuing Region) that would let this check happen. Note this does not touch the rule that the last page reports the EMPTY `NextToken` string (fixed issue 20261028T190444Z): the token value and the empty-string-on-exhaustion contract stay exactly as they are; only a token whose issuing endpoint differs from the requesting one needs refusing the way a foreign-kind token already is.

## Evidence

Token contents carry no Region — `services/.../src/state.rs:1671`:
```
pub fn encode_page_token(kind: &str, key: &str) -> String {
    ::hex::encode(format!("{kind}\u{1f}{key}"))
```
and the decode only checks the kind tag — `state.rs:1677`:
```
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    ...
    decoded.strip_prefix(&format!("{kind}\u{1f}")).map(str::to_string)
```

The handler resumes from that key with no Region check — `src/ops/list_configuration_managers.rs:181-183` then `:203-205`:
```
        let start = match crate::state::resume_token(input.starting_token.as_deref()) {
            Some(token) => Some(
                crate::state::decode_page_token(crate::state::MANAGER_PAGE_TOKEN_KIND, token)
...
        let (page, next_token) = crate::state::paginate(
            crate::state::MANAGER_PAGE_TOKEN_KIND,
            store.managers.range(start.unwrap_or_default()..),
```
while the same handler states the contract it breaks (`list_configuration_managers.rs:155-157, 177-179`):
```
        // Quick Setup is a regional control plane, so this listing reports the managers
        // created through the endpoint of the Region this request was signed for and no
        // others: a manager of another Region is not this Region's resource.
...
        // A token this operation did not issue is refused rather than silently read as
        // "start from the beginning", which would re-serve a page the caller already had.
```

The crate's own invariant that the quoted `paginate` is supposed to hold — `state.rs:1609-1610`:
```
/// Either way the token names where to resume,
/// so paging still visits every record exactly once.
```
and the centralised scoping every other identifier path goes through — `state.rs:1255-1257`:
```
/// one place that scoping lives, so every ARN-addressed operation treats an ARN of
/// another Region exactly as it treats one that was never minted.
```

Test coverage: `tests/all/pagination_tokens.rs` exercises both listings' tokens within ONE signed Region only (its `LISTINGS` table and `page_of` helper send no per-Region credential variation), and `tests/all/region_scoping.rs` only asserts the tokenless case — "A request signed for any other Region lists none of it (its page is empty and its `NextToken` is the empty string...)" (`region_scoping.rs:12-14`). No test replays one Region's token on another Region's request, so nothing pins the current behaviour.

## Steps to Reproduce

1. Boot the emulator with the ssm-quicksetup service mounted.
2. Sign two CreateConfigurationManager calls for us-west-2 (`POST /configurationManager`, each with one valid ConfigurationDefinitions entry naming TargetRegions and TargetOrganizationalUnits), so that Region holds two managers.
3. Sign `POST /listConfigurationManagers` for us-west-2 with `{"MaxItems": 1}` and keep the non-empty `NextToken` T it returns (T hex-decodes to `configuration-manager\u{1f}arn:aws:ssm-quicksetup:us-west-2:...`).
4. Sign one CreateConfigurationManager call for us-east-1, so that Region holds a manager too; confirm `POST /listConfigurationManagers` signed for us-east-1 with `{}` reports it.
5. Now sign `POST /listConfigurationManagers` for us-east-1 with `{"StartingToken": T}`.
6. Observed: 200 with an empty `ConfigurationManagersList` and `NextToken: ""` — the us-east-1 manager is silently dropped from the listing, and no further page can reach it. Expected: the token is not one this endpoint issued, so it is refused the way a foreign-kind token is (400 ValidationException), rather than answering a page that omits the requesting Region's own records.
