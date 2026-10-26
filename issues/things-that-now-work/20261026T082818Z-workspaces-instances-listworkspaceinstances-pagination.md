Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# A NextToken this service never issued is honoured as a resume point, so ListWorkspaceInstances answers 200 with an empty page instead of refusing it

_Filed: 2026-10-26T08:28:18Z_
_Operation: ListWorkspaceInstances | Class: pagination_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListWorkspaceInstances validates a caller's `NextToken` only against the SHAPE of a `WorkspaceInstanceId`, not against anything this service issued. Any hex-encoded string that happens to satisfy `^wsinst-[0-9a-zA-Z]{8,63}$` is therefore accepted as the resume point and the listing seeks straight to it: `state.instances.range(resume..)`. Because the minted ids are `wsinst-{n:016x}` (state.rs:1881), a fabricated token such as `hex("wsinst-ffffffffffffffff")` sorts after every stored id, so the handler answers HTTP 200 with `WorkspaceInstances: []` and no `NextToken` — a caller that passes a token it did not get from this service (a token from another account/list, a truncated or re-encoded one) is told the Region holds no instances instead of being refused. A lower fabricated id silently skips every record before it.

This contradicts (a) the handler's own comment directly above the check, (b) the doc of the shared decoder in state.rs, and (c) the sibling paginated operation `ListInstanceTypes`, which resolves the decoded resume point against the list it is paging (`instance_type_index`) and refuses a token naming a row it does not hold. The crate therefore already has the "resolve the resume point, do not guess" shape in one list handler and skips it in the other, and turns a bad request into a successful (wrong) answer.

## Evidence

Handler comment states the rule the code under it does not implement — services/service-com-amazonaws-workspacesinstances-eucmifrontendapiservice/src/ops/list_workspace_instances.rs:158-167:
```
        // The `NextToken` this operation mints carries the `WorkspaceInstanceId` the
        // next page resumes at, in this service's one token encoding
        // ([`crate::state::decode_next_token`]): a token must decode to a value shaped
        // like an instance id, so one this service did not issue is refused rather than
        // guessed at.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => {
                let decoded = crate::state::decode_next_token(token)
                    .filter(|id| crate::state::is_workspace_instance_id(id));
```
The only test is SHAPE-only: that filter calls `is_workspace_instance_id`, whose own doc says it checks shape alone — state.rs:2117-2118: "/// Whether `candidate` is shaped like a `WorkspaceInstanceId`: `len 15..70` and /// `^wsinst-[0-9a-zA-Z]{8,63}$`".

The accepted value is then used as a seek key — list_workspace_instances.rs:197-200:
```
        let from_resume = match &resume_from {
            Some(resume) => state.instances.range::<String, _>(resume..),
            None => state.instances.range::<String, _>(..),
```
with ids minted as state.rs:1881 `(format!("wsinst-{n:016x}"), format!("i-{n:017x}"))`, so `wsinst-ffffffffffffffff` is past every stored id and the page comes back empty with `next_token = None` (lines 216-236).

Shared decoder doc states the stronger rule — state.rs:2182-2189:
```
/// This service's one pagination-token encoding: the resume point's own identifier,
/// hex-encoded. Hex carries no secrecy — it is trivially reversible — it is a
/// round-trip and validation device, so a token that does not decode (or decodes to
/// a resume point the caller's list cannot name) is refused with
/// [`invalid_next_token`] rather than guessed at. Both paginated operations that mint
/// a token use this, so the scheme lives in one place.
```
Sibling that DOES resolve the resume point — src/ops/list_instance_types.rs:185-195:
```
                let decoded = crate::state::decode_next_token(token)
                    .and_then(|name| crate::state::instance_type_index(&name));
                match decoded {
                    Some(index) => index,
                    None => {
                        return Err(ListInstanceTypesErrorModeled::ValidationException(
                            crate::state::invalid_next_token(),
```
Test coverage: tests/all/workspace_instance_lifecycle.rs:1022 `a_next_token_this_service_did_not_issue_is_refused` asserts the rule by name but every one of its seven rows is a SHAPE failure ("not-a-token", odd-length hex, non-UTF-8 hex, a raw id, `hex("zzzzzzzzzzzzzz")`, `hex("vol-0123456789")`, `hex("wsinst-1234567")` — lines 1027-1037); no row is a well-shaped id this service never minted, which is the case that slips through. The sibling's equivalent test does cover it: tests/all/instance_type_catalog.rs:441-442 "// Valid hex decoding to an instance type the catalog does not hold." expects HTTP 400.

## Steps to Reproduce

1. Create two WorkSpace Instances with CreateWorkspaceInstance in us-east-1 (ids `wsinst-0000000000000001`, `wsinst-0000000000000003`).
2. Call ListWorkspaceInstances with no NextToken and observe both instances are reported.
3. Call ListWorkspaceInstances with `NextToken` = hex-encoding of the string `wsinst-ffffffffffffffff` (i.e. `7773696e73742d66666666666666666666666666666666`) — a token this service never minted.
4. Observe HTTP 200 with `WorkspaceInstances: []` and no `NextToken`, rather than the HTTP 400 ValidationException the handler comment (list_workspace_instances.rs:158-162) and state.rs:2182-2189 say an unissued token gets, and that ListInstanceTypes answers for the same class of token (tests/all/instance_type_catalog.rs:441-450).
