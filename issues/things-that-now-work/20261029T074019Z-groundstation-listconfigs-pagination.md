Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# A nextToken carrying a field list this listing never writes is honoured as a page position instead of refused, and can hide every record

_Filed: 2026-10-29T07:40:19Z_
_Operation: ListConfigs | Class: pagination_

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::decode_page_token` is the one gate every listing of this service puts a caller's `nextToken` through. It strips the scope prefix and parses each dot-separated field, but it never checks that the field list it decoded is one the listing in question actually WRITES: not the number of fields, and not the form (`Absent`/`Number`/`Text`) of each. So a token that merely starts with the listing's scope decodes to `Some(cursor)` and is then used as a real page boundary.

Four listings of this crate take their scope straight from a fixed, guessable constant rather than from the filter fingerprint `page_scope` hashes — `ListConfigs` (`"configs"`), `ListMissionProfiles` (`"missionprofiles"`), `ListDataflowEndpointGroups` (`"dataflowendpointgroups"`) and `ListSatellites` (`"satellites"`) — so for those the forgeable prefix is public. `ListConfigs` writes a TWO-field cursor (`[Text(configType), Text(configId)]`), yet:

* `nextToken=configs.n1` decodes to the one-field cursor `[Number(1)]`. `CursorField` derives `Ord` with `Absent < Number < Text`, so every item's `[Text(..), Text(..)]` sorts ABOVE it, `paginate`'s `partition_point` answers 0, and the request is served HTTP 200 with the first page — plus a fresh `nextToken` — where the handler's own refusal says the token was not issued by a previous `ListConfigs` call.
* `nextToken=configs.t7a7a7a` decodes to `[Text("zzz")]`. Every stored config's `configType` (`antenna-downlink`, `s3-recording`, `tracking`, `uplink-echo`, …) sorts BELOW `"zzz"`, so `partition_point` returns `items.len()`: the answer is HTTP 200 with an empty `configList` and NO `nextToken` — a caller is told this account holds no configs at all.

Both tokens satisfy the modelled `PaginationToken` shape (`^[A-Za-z0-9-/+_.=]+$`, len 3..1000), so they are input the model admits and the boundary validator passes them through to the handler. The failure mode is an error turned into a success: the modelled `InvalidParameterException` (HTTP 431) the handler composes for an unissued token is never reached, and in the second case every record of the listing is silently dropped from the answer.

## Evidence

The helper promises exactly what it does not do — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:1171`:

    /// The cursor a token issued by [`encode_page_token`] for `scope` carries, or `None` when
    /// the token was not issued by this service for that scope.
    pub fn decode_page_token(scope: &str, token: &str) -> Option<PageCursor> {
        let fields = token.strip_prefix(scope)?.strip_prefix('.')?;
        fields
            .split('.')
            .map(CursorField::parse)
            .collect::<Option<Vec<CursorField>>>()
            .map(PageCursor::new)

Nothing there compares the decoded field list with the cursor the listing writes, so a one-field `n…` or `t…` token for a two-field listing decodes to `Some`.

The order that makes the forged cursor usable — `state.rs:1055`:

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    pub enum CursorField {
        /// An optional order member that carries nothing.
        Absent,
        /// A number, ordered as numbers are. ...
        Number(i64),
        /// Text, ordered as `str` orders text ...
        Text(String),
    }

and `paginate` then trusts it — `state.rs:1216`:

        let start = match after {
            None => 0,
            // Everything up to and including the item the previous page ended on is behind the
            // boundary; the sequence is ascending, so that prefix is found by bisection.
            Some(after) => items.partition_point(|item| key_of(item) <= *after),
        };

The handler whose promise is broken — `src/ops/list_configs.rs:140` and `:169`:

        const LISTING: &str = "configs";
...
            Some(token) => Some(crate::state::decode_page_token(LISTING, token).ok_or_else(
                || {
                    ListConfigsErrorModeled::InvalidParameterException(InvalidParameterException {
                        message: Some(
                            "nextToken was not issued by a previous ListConfigs call".to_string(),
                        ),

and the two-field cursor it actually issues, `src/ops/list_configs.rs:190`:

            |config| {
                crate::state::PageCursor::new(vec![
                    crate::state::CursorField::optional_text(config.config_type.as_deref()),
                    crate::state::CursorField::optional_text(config.config_id.as_deref()),
                ])
            },

The crate states the rule the forged token escapes, in its own test — `tests/all/antenna_inventory.rs:341` (`list_antennas_refuses_a_next_token_it_did_not_issue`):

    // A token addresses an offset into the sequence that issued it, so this listing accepts
    // only its own — for THIS ground station — and refuses anything else rather than
    // resuming at an offset into an unrelated sequence.

That test's five refused cases all fail either the scope prefix (`not-a-token`, `antennas.1`, another listing's token, another ground station's token) or `CursorField::parse` (`antennas.not-a-number`). No existing test covers a token that carries the right scope and a field list of the wrong arity or the wrong field form, which is the hole above.

## Steps to Reproduce

1. Create two or more configs with CreateConfig (e.g. a trackingConfig and an s3RecordingConfig), so ListConfigs has records to page.
2. GET /config — observe HTTP 200 and every config in configList.
3. GET /config?nextToken=configs.n1 — expected the handler's modelled 431 InvalidParameterException with message "nextToken was not issued by a previous ListConfigs call" and parameterName "nextToken" (this token was issued by nothing; ListConfigs only ever writes two-field `configs.t<hex>.t<hex>` tokens). Actual: HTTP 200 with the first page of configList, because decode_page_token returns Some([Number(1)]) and every item's [Text, Text] cursor sorts above it.
4. GET /config?nextToken=configs.t7a7a7a (the hex of "zzz" in one Text field) — expected the same 431. Actual: HTTP 200 with an empty configList and no nextToken, because every stored configType sorts below "zzz" and paginate's partition_point skips the whole sequence — the caller is told the account holds no configs.
5. Repeat against GET /missionprofile with nextToken=missionprofiles.n1, GET /dataflowEndpointGroup with nextToken=dataflowendpointgroups.n1 and GET /satellite with nextToken=satellites.t7a7a7a: each of those listings also takes its scope from a fixed constant, so each honours a token it never issued.
