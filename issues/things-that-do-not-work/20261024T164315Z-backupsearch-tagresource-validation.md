Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# Tag keys and values with characters outside the documented set are accepted instead of refused with ValidationException

_Filed: 2026-10-24T16:43:15Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents the character set a BackupSearch tag may use: TagResourceRequest$Tags is "a key-value pair you can use to manage, filter, and search for your resources. Allowed characters include UTF-8 letters, numbers, spaces, and the following characters: + - = . _ : /." (the same sentence is on StartSearchResultExportJobInput$Tags and StartSearchJobInput$Tags).

Expected: a TagResource call whose Tags carry a key or value containing a character outside that set (e.g. "bad*key", "env#1", "owner@corp", "a,b") is refused 400 ValidationException and nothing is written, so a later ListTagsForResource does not report the rejected entry.

Actual: the emulator validates only LENGTH and COUNT. `crate::state::check_tag_entry` checks the key against MAX_TAG_KEY_LENGTH and the value against MAX_TAG_VALUE_LENGTH and returns Ok for anything else; `check_tag_count` checks the merged entry count. No character-class check exists anywhere in the crate. So TagResource answers its empty success (204) for a tag key/value AWS rejects, the entry is persisted into the resource's tag map, and ListTagsForResource reports it afterwards — a write AWS never performed becomes observable state.

The same unchecked path is shared by the two creating operations, which call the identical helpers before storing the record: StartSearchJob (src/ops/start_search_job.rs, the `for (key, value) in &tags { check_tag_entry(..) }` loop before `insert_job`) and StartSearchResultExportJob (src/ops/start_search_result_export_job.rs, same loop before `insert_export_job`) both mint a job carrying an illegal tag. Please close the character-set guard once in the shared helper rather than per handler.

## Evidence

Spec: smithy_query(query="doc", shape="TagResourceRequest", member="Tags") returns "Required tags to include. A tag is a key-value pair ... Allowed characters include UTF-8 letters, numbers, spaces, and the following characters: + - = . _ : /." — a documented input constraint, and TagResource declares the service-common ValidationException (httpError 400). The same doc sentence appears on StartSearchResultExportJobInput$Tags.

Handler: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/tag_resource.rs:178-192 — the only input checks are `crate::state::check_tag_entry(key, value)` per entry and `crate::state::check_tag_count(merged)`, then `stored.insert(key, value)` unconditionally. src/state.rs `check_tag_entry` (documented as "Refuse one tag entry whose key or value is longer than this store's cap") compares `key.chars().count()` to MAX_TAG_KEY_LENGTH=128 and `value.chars().count()` to MAX_TAG_VALUE_LENGTH=256 and returns Ok(()) otherwise — no pattern/character-class test. No other validation of tag content exists in the crate (grep for the documented characters finds only test comments).

Test coverage: the existing suites assert only the POSITIVE direction. tests/all/tag_removal.rs:43-44 and tests/all/resource_tags.rs:102-103 state that their keys "exercise the character classes `Tags` documents for this service (\"UTF-8 letters, numbers, spaces, and the following characters: + - = . _ : /\")" — i.e. they check that ALLOWED characters round-trip. tests/all/search_jobs.rs:1217-1261 covers the tag count and key/value LENGTH caps (ValidationException one past the cap) but no disallowed character. Nothing exercises a key or value outside the documented set, and no existing open BackupSearch issue covers tag content validation (the filed TagResource issues cover the 204 success status and the not-found error code only).

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. start_search_job(Name="t", SearchScope={"BackupResourceTypes":["S3"]}) and keep the returned SearchJobArn.
3. tag_resource(ResourceArn=<that ARN>, Tags={"bad*key": "v"}) — the emulator answers an empty success (204); AWS refuses the call with 400 ValidationException for the disallowed character in the key.
4. list_tags_for_resource(ResourceArn=<that ARN>) — the emulator reports {"bad*key": "v"}, i.e. the rejected entry is now persisted state.
5. Repeat with an illegal VALUE, e.g. tag_resource(ResourceArn=<ARN>, Tags={"env": "a@b,c"}) — also accepted.
6. Positive control: tag_resource(ResourceArn=<ARN>, Tags={"env+1 x": "a-b_c:d/e.f=g"}) uses only documented characters and must keep succeeding.
7. Same root cause at create time: start_search_job(Name="t", SearchScope={"BackupResourceTypes":["S3"]}, Tags={"bad*key":"v"}) returns 200 and mints a job carrying the illegal tag; likewise start_search_result_export_job(..., Tags={"bad*key":"v"}).
