Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 032f9a5bcc1a20e2d342a3403d3d533abd316530

# DeleteLensShare revokes a custom lens's share named by the BARE lens id — a name CreateLensShare and every other lens operation of this crate refuses as an AWS-official lens

_Filed: 2026-10-29T10:32:16Z_
_Operation: DeleteLensShare | Class: validation_

> _Note: this issue was identified against the code base at commit `032f9a5bcc1a20e2d342a3403d3d533abd316530`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate decides whether a `LensAlias` names a custom lens or an AWS-official one in ONE place, `crate::state::is_aws_official_lens`, whose documented rule is that a name that is not an ARN can only be an official lens: "A bare alias is official too — a custom lens is only ever named by its ARN." Seven lens operations apply it before they touch the lens store and refuse a bare name accordingly: `CreateLensShare` (400 "Member must be the ARN of a custom lens…"), `ExportLens`, `DeleteLens`, `CreateLensVersion`, `GetLens`, `GetLensVersionDifference`, `ImportLens`. The state module states the same rule for the tag plane — "a bare lens id is not an ARN, so it names no resource here" — and a test row pins it (`ListTagsForResource` on the bare lens id of an existing custom lens must 404).

`DeleteLensShare` applies no such classification. It resolves the lens with `store.lens_by_alias(&input.lens_alias)`, and `lens_by_alias` keys the store by the lens ID and then accepts the name through `alias_names_lens`, whose non-ARN branch is `alias == record.lens_id` — so the BARE lens id resolves to the stored custom lens and the share is removed. The same request body is refused 400 by `CreateLensShare`, so a caller can REVOKE a lens share with a name it cannot CREATE one with, and the handler's own comment grounds its lookup on `CreateLensShare`'s rule ("a STORED CUSTOM lens is the only kind that can hold a share, because 'Lenses provided by Amazon Web Services … cannot be shared' (`CreateLensShare` doc)") while classifying the name the opposite way from that very operation.

The read sibling `ListLensShares` has the identical hole (`src/ops/list_lens_shares.rs:221-241`): it lists the custom lens's shares for the bare id, although `GetLens` answers 404 for the same name — so one fix covers both. The write path is reported here because it is the one that changes state: a share that `CreateLensShare` records under the lens's ARN is destroyed by a name this crate says names no custom lens.

## Evidence

Model — the spelling a custom lens is named by, `smithy_query(query="shape", shape="LensAlias")` / quoted in the handler headers (e.g. `src/ops/export_lens.rs:59-65`): "For Amazon Web Services official lenses, this is either the lens alias, such as serverless, or the lens ARN … For custom lenses, this is the lens ARN, such as arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef."

Side 1 — the crate's one classification rule, `src/state.rs:4095-4111`:
```
/// Whether a `LensAlias` names an AWS-official lens rather than a custom one.
/// … an official lens ARN names NO owner account … while a custom lens ARN
/// carries its owner's account id. A bare alias is official too — a custom lens is only
/// ever named by its ARN.
pub fn is_aws_official_lens(alias: &str) -> bool {
    let Some(arn) = alias.strip_prefix("arn:") else {
        return true;
    };
```
and the sibling that applies it to this exact share pair, `src/ops/create_lens_share.rs:191-204`:
```
// "Lenses provided by Amazon Web Services (Amazon Web Services Official Content)
// cannot be shared" (this operation's doc). … An official lens is the one
// named by a bare alias or by an ARN carrying no owner account
// (`crate::state::is_aws_official_lens`) …
if crate::state::is_aws_official_lens(&input.lens_alias) {
    return Err(field_validation(
        LENS_ALIAS_MEMBER,
        "Member must be the ARN of a custom lens; a lens provided by Amazon Web \
         Services cannot be shared",
    ));
}
```

Side 2 — the delete path, with no classification at all, `src/ops/delete_lens_share.rs:257-282`:
```
// The share belongs to the lens the request names, so that lens has to resolve
// first — and a STORED CUSTOM lens is the only kind that can hold a share, because
// "Lenses provided by Amazon Web Services (Amazon Web Services Official Content)
// cannot be shared" (`CreateLensShare` doc). …
let lens_id = match store
    .lens_by_alias(&input.lens_alias)
    .map(|record| record.lens_id.clone())
{
    Some(lens_id) => lens_id,
```
followed by the removal at `src/ops/delete_lens_share.rs:293-298` (`if store.remove_lens_share(&lens_id, &input.share_id).is_none() { … }`). Why the bare id resolves: `src/state.rs:6082-6084` — "Every stored custom lens, keyed by its 32-lowercase-hex lens id" / `pub lenses: HashMap<String, LensRecord>,` — plus `src/state.rs:6806-6809`:
```
pub fn lens_by_alias(&self, alias: &str) -> Option<&LensRecord> {
    let record = self.lenses.get(lens_alias(alias))?;
    alias_names_lens(alias, record).then_some(record)
}
```
and `src/state.rs:4082-4093`:
```
/// Whether `alias` names `record`: … a bare id names the lens whose id it is.
fn alias_names_lens(alias: &str, record: &LensRecord) -> bool {
    if !alias.starts_with("arn:") {
        return alias == record.lens_id;
    }
```

Side 2b — the crate stating the OPPOSITE rule for the same bare id, `src/state.rs:6681-6685`: "A lens is resolved through [`Self::lens_by_alias`], which requires the ARN to agree with the lens's own (account and Region included) … and it is consulted only for a string that IS an ARN: a bare lens id is not an ARN, so it names no resource here", pinned by `tests/all/resource_tags.rs:416-427`: "// The bare lens id of the lens that exists: a lens is named by its ARN, so its id on its own names no resource to report the tags of." → `rows.push(("a bare lens id, not an ARN", bare_lens_id, None));` asserted 404 at `:429-438`.

Same hole on the read sibling, `src/ops/list_lens_shares.rs:217-241`: "the lens is resolved by this lookup" → `store.lens_by_alias(&input.lens_alias).map(|record| record.lens_id.clone())`, with the only `is_published_official_lens` consultation in the MISS arm (`:231`).

Test coverage that stays unchanged: `tests/all/lens_share_deletion.rs:336-374` (`a_delete_that_resolves_nothing_is_refused_and_removes_nothing`) walks "an entry of that catalog this suite is not otherwise about, the official-ARN spelling of another, an alias outside the catalog, an unknown custom ARN, and share ids belonging to another lens and to none" — no row sends the BARE id of a custom lens this store holds, so nothing pins the behaviour reported here; `tests/all/lens_shares.rs` and `tests/all/lens_share_listing.rs` name the lens by its ARN throughout.

## Steps to Reproduce

1. ImportLens a custom lens and CreateLensVersion to publish it; note the minted ARN arn:aws:wellarchitected:us-west-2:<account>:lens/<lensId>
2. POST /lenses/arn%3Aaws%3Awellarchitected%3Aus-west-2%3A<account>%3Alens%2F<lensId>/shares with {"SharedWith": "666666666666", "ClientRequestToken": "t1"} — 200 with ShareId S
3. POST /lenses/<lensId>/shares (the BARE lens id) with the same body — 400 ValidationException "Member must be the ARN of a custom lens; a lens provided by Amazon Web Services cannot be shared" from src/ops/create_lens_share.rs:198-204, because is_aws_official_lens("<lensId>") is true (src/state.rs:4104-4111)
4. DELETE /lenses/<lensId>/shares/S?ClientRequestToken=t2 — the BARE lens id this crate just called an official lens — and observe 200: src/ops/delete_lens_share.rs:264-266 resolves it through lens_by_alias's non-ARN branch (src/state.rs:4086-4088) and src/ops/delete_lens_share.rs:293 removes the share
5. GET /lenses/<ARN>/shares no longer lists share S: a share created only under the ARN spelling was destroyed by a name the create refuses
6. GET /lenses/<lensId>/shares also answers 200 with that lens's shares (src/ops/list_lens_shares.rs:221-225), while GET /lenses/<lensId> answers 404 ResourceNotFoundException for the same name (src/ops/get_lens.rs:157 and :257-261) and ListTagsForResource on it answers 404 per tests/all/resource_tags.rs:416-438
