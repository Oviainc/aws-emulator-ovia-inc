Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0184958fe7a8a2f43d9a0a5d46b055181f8d4624

# decode_next_token honours position spellings encode_next_token never writes ("+N", leading zeros), so a token this service did not issue is served as a page

_Filed: 2026-10-29T09:27:58Z_
_Operation: ListSearchJobBackups | Class: pagination_

> _Note: this issue was identified against the code base at commit `0184958fe7a8a2f43d9a0a5d46b055181f8d4624`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginating listing of this crate (ListSearchJobBackups, ListSearchJobResults, ListSearchJobs, ListSearchResultExportJobs) resolves its `NextToken` through the single helper `crate::state::decode_next_token`, whose own documentation states the rule it is there to enforce: a token this service did not issue must be an `Err` (which the handlers lift into a 400 ValidationException). The token this service DOES issue is minted by the matching `encode_next_token`, which renders the position with `format!("{position}@{context}")` — i.e. a `usize` in canonical decimal: no sign character and no leading zeros.

The decode side does not hold the position to that spelling. It parses the text before the `@` with `str::parse::<usize>()`, which additionally accepts an optional leading `+` and any number of leading zeros. So `"+3@backups:36:<job-id>"` and `"003@backups:36:<job-id>"` are accepted as page positions, and the listing answers 200 with a page, although `encode_next_token` can never have produced either string. The handler's refusal path for a token the service never minted — the one the crate's own test pins for five other spellings — is therefore bypassed for this family of spellings, turning a documented error into a success.

The defect is in the shared helper, so all four listings behave the same way; ListSearchJobBackups is named as the first checklist operation whose handler calls it (`list_search_job_backups.rs:209`).

## Evidence

The rule, stated by the helper itself — services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/state.rs:2145-2146:
```
/// `Err` is the caller's problem: a token this service did not issue, one issued for a
/// different sequence, or one that points outside what it is paging over.
```
and the refusal it builds, state.rs:2156-2157:
```
    let malformed =
        || format!("NextToken '{token}' is not a pagination token this service issued");
```

What the service issues — state.rs:2061-2062:
```
pub fn encode_next_token(context: &str, position: usize) -> String {
    format!("{position}@{context}")
}
```
(`{position}` of a `usize` is canonical decimal: no `+`, no leading zeros.)

What the service accepts — state.rs:2164:
```
    let position: usize = position.parse().map_err(|_| malformed())?;
```
`usize::from_str` accepts a leading `+` and leading zeros, so `"+3"` and `"003"` parse to 3 and reach `anchor.offset(len, position)` (state.rs:2165) as a valid cursor.

Caller side (all four listings share the helper): list_search_job_backups.rs:209-215 `crate::state::decode_next_token(input.next_token.as_deref(), &context, job.backups.len(), crate::state::PageAnchor::CreationOrder).map_err(crate::scaffolded::Scaffolded::validation)?`; identical calls in list_search_job_results.rs:194-200, list_search_jobs.rs:186-192, list_search_result_export_jobs.rs:216-222.

Test coverage proving the rule is pinned but this spelling family is not covered — tests/all/search_jobs.rs:668-700:
```
async fn a_next_token_this_service_did_not_issue_is_refused() {
...
    for token in ["not-a-cursor", "9999", "-1", "9999@jobs:ALL", "1@jobs:RUNNING"] {
...
            listed.status, 400,
            "case {token:?}: a token this service did not issue for THIS request must be refused, got {} ({:?})",
```
A grep of the whole `tests/` tree for `+`-signed or leading-zero cursor cases returns nothing, so no test states a reason for admitting them.

## Steps to Reproduce

1. Boot the emulator and point a restJson1 backupsearch client at it.
2. PUT /search-jobs with {"SearchScope": {"BackupResourceTypes": ["S3"]}} and note the returned SearchJobIdentifier.
3. GET /search-jobs/{SearchJobIdentifier}/backups?NextToken=%2B0@backups:36:{SearchJobIdentifier} (a `+`-signed position) — the call answers 200 with a page instead of the 400 ValidationException the helper's documented rule ("a token this service did not issue") calls for; `encode_next_token` can only ever have written `0@backups:36:{SearchJobIdentifier}`.
4. Repeat with NextToken=00@backups:36:{SearchJobIdentifier} (leading zeros) — same 200.
5. Contrast with tests/all/search_jobs.rs:668-700, which asserts 400 + ValidationException for the five spellings it enumerates, and with the same listing's response to `9999@...` (refused), showing the intended classification.
