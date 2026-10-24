Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# UntagResource answers HTTP 200 where the service answers 204 for its empty output

_Filed: 2026-10-24T09:35:48Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UntagResource (DELETE /tags/{ResourceArn}) is modeled with an EMPTY output structure (UntagResourceResponse has no members) and with NO explicit @http(code): smithy_query reports `"code": 200, "codeDefaulted": true`, i.e. the model states nothing about the success status, so the service's own wire behavior is the authority.

For the sibling operation on the same resource and the same empty-output shape — TagResource, POST /tags/{ResourceArn}, also codeDefaulted — the recorded AWS response to a successful call is HTTP 204 with no content (already filed as cause 5 of 20261022T215802Z-backupsearch-refusal-fidelity-on-the-wire-not-found.md, whose replay key is `backupsearch|TagResource|2xx->2xx|status`). UntagResource is the same shape of call on the same route with the same empty output and the same defaulted code, so a successful untag answers 204 as well; the emulator hardcodes 200.

Expected: a successful UntagResource answers HTTP 204 with an empty body.
Actual: it answers HTTP 200 with an empty body.

Note for the fixer: the existing TagResource issue names only `the TagResource handler's success serialization`, and its replay keys carry nothing for UntagResource, so fixing TagResource alone leaves this operation on 200. StopSearchJob's empty output is NOT affected — the model gives StopSearchJob an explicit `-> 200` (see OPERATIONS.md), unlike the three tag operations.

## Evidence

Spec: smithy_query(query="operation", operation="UntagResource") -> output `{}` (UntagResourceResponse, no members), `http: {uri: "/tags/{ResourceArn}", method: "DELETE", code: 200, codeDefaulted: true}`; TagResource is modeled identically (empty output, codeDefaulted 200) and its recorded AWS success is 204. Handler: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/untag_resource.rs:136 — `const SUCCESS_STATUS: u16 = 200u16;` and the handler returns `Ok(UntagResourceOutput {})`. Test coverage: tests/all/tag_removal.rs:256,301,340 and tests/all/resource_tags.rs:367,436 all assert `untagged.status == 200`, i.e. the emulator's 200 is pinned by tests and nothing asserts 204; those assertions have to move with the fix.

## Steps to Reproduce

1. Boot the emulator and point a botocore backupsearch client at it.
2. start_search_job(Name="x", SearchScope={"BackupResourceTypes":["S3"]}) and keep SearchJobArn.
3. tag_resource(ResourceArn=<arn>, Tags={"k":"v"}).
4. untag_resource(ResourceArn=<arn>, TagKeys=["k"]) and inspect ResponseMetadata.HTTPStatusCode: the emulator reports 200, AWS reports 204 (no content) for this empty-output operation.
5. list_tags_for_resource(ResourceArn=<arn>) still correctly shows the key removed — only the success status diverges.
