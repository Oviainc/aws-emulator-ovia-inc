Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 15a7913299c0ebfe5f22c92412d933bd37150a6d

# A ListMetadataTransferJobs nextToken is scoped to the transfer's two ends but not to the `filters` that selected its page

_Filed: 2026-10-29T16:13:51Z_
_Operation: ListMetadataTransferJobs | Class: pagination_

> _Note: this issue was identified against the code base at commit `15a7913299c0ebfe5f22c92412d933bd37150a6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The listing's page is the jobs that match the requested `sourceType`/`destinationType` AND every entry of `filters` (`workspaceId`, `state`). The token is bound only to the two end types, so a token minted under one filter set is accepted by a request carrying different filters (or none). The resume is a `range(from..)` over the map, applied before the new filters are evaluated, so every job that satisfies the new filters but whose `metadataTransferJobId` sorts BEFORE the token's id is silently omitted — the caller gets 200 and an incomplete walk.

The handler's own comment claims a token whose page "would mean something else" is refused, and the crate's other paged query (GetPropertyValue) binds the row-selecting conditions into the token scope for exactly this reason. `filters` is left out of the scope here.

## Evidence

The token scope is the two ends only — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_metadata_transfer_jobs.rs:500-510:
```
fn encode_next_token(
    source_type: &str,
    destination_type: &str,
    metadata_transfer_job_id: &str,
) -> String {
    crate::state::encode_page_token(
        NEXT_TOKEN_PREFIX,
        &[source_type, destination_type],
        metadata_transfer_job_id,
    )
}
```
while the handler asserts the broader rule — list_metadata_transfer_jobs.rs:275-280:
```
// A `nextToken` carries the `metadataTransferJobId` the page resumes AT
// (inclusive), not an offset, so a page boundary survives a job being
// created between two pages. It is bound to the ends the listing was
// taken over, so a token from another listing — or from this listing
// taken over other ends, whose page would mean something else — is
// refused rather than silently resumed.
```
and the filters are what decide the sequence — list_metadata_transfer_jobs.rs:333-341:
```
let mut matching = match &resume_at {
    Some(from) => state.metadata_transfer_jobs.range(from.clone()..),
    None => state.metadata_transfer_jobs.range::<String, _>(..),
}
.map(|(_, job)| job)
.filter(|job| {
    transfers_between(job, &input.source_type, &input.destination_type)
        && filters.iter().all(|filter| matches_filter(job, filter))
});
```
The sibling paged query in the SAME crate binds the row-selecting members — src/ops/get_property_value.rs:1149-1153: "the conditions that decide which rows there are and in what order ([`conditions_scope`]), so a token is only resumed by the query that minted it" — and src/state.rs:4501-4506: "a listing that resumes at one binds its token to every request member that decides that sequence — and a filter decides it, since a value no filter selects is not in it."

Test coverage: tests/all/metadata_transfer_job_listing.rs:648-651 refuses "a nextToken minted over other ends" and :666-678 asserts "a token keeps resuming the listing it was minted for" for the same ends; no case changes `filters` across a resume, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateMetadataTransferJob three times with ids `job-a`, `job-b`, `job-c`, each with an `s3` source and an `iottwinmaker` destination naming `w`.
3. ListMetadataTransferJobs with `{"sourceType":"s3","destinationType":"iottwinmaker","maxResults":1,"filters":[{"state":"VALIDATING"}]}` — one job is reported and a nextToken resuming at `job-b` comes back.
4. Re-issue the call with the same ends, `maxResults: 10`, that nextToken and NO `filters`.
5. Observed: 200, and `job-a` — a member of the unfiltered listing that sorts before the token's id — never appears in the walk.
6. Expected (per the handler's own comment at list_metadata_transfer_jobs.rs:275-280 and the token-scope rule at get_property_value.rs:1149-1153): a request whose `filters` differ from the ones the token was minted under is refused with the modelled ValidationException rather than resumed.
