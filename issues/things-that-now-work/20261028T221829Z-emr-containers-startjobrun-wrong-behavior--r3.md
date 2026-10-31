Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ebd02413e29cf57b2ef3e9c9311b8c37c19f24b

# An EMPTY jobTemplateParameters map is refused where an omitted one is accepted, against this crate's own "empty collection == omitted" rule

_Filed: 2026-10-28T22:18:29Z_
_Operation: StartJobRun | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ebd02413e29cf57b2ef3e9c9311b8c37c19f24b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartJobRun`'s no-template branch refuses the submission whenever `jobTemplateParameters` is merely PRESENT (`input.job_template_parameters.is_some()`), so a request that carries `"jobTemplateParameters": {}` is answered 400 `ValidationException` while the byte-for-byte equivalent request that simply omits the member is accepted and starts a run.

That contradicts this crate's own, explicitly written rule that an omitted collection and an empty one are the SAME request: `state::RequestRendering` exists partly to erase the difference (`collapse_empty`), and `state::request_fingerprint` is documented as making the two spellings digest identically. So the two bodies are, by the crate's own definition of request identity, one request that now gets two different answers — one 200 with a job run, one 400 — and under one `clientToken` the idempotency index cannot even tell them apart.

The handler's own comment states the reason the check exists — parameters "would be silently dropped ... there is nothing for them to be substituted into" — which does not apply to an EMPTY map: it supplies no parameter, so nothing is dropped by accepting it. Every sibling in this crate normalises an empty collection to "absent" for exactly that reason: `state::within_listed_states` filters `!states.is_empty()` before constraining, `list_managed_endpoints::matches_filters` does the same for `types`, `state::configured_scheduler_limits` collapses a structure whose every member is omitted, and `StartJobRun` itself does `input.tags.clone().unwrap_or_default()` three lines later. Only this one check uses a bare `.is_some()`.

The member is modelled `len 0..100`, so the empty map is a value the model admits and the boundary passes through; the codec assigns `Some(<empty map>)` for a present member (which is precisely why `collapse_empty` has anything to do), so the branch is reachable from an ordinary JSON body.

## Evidence

The refusal — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs:283-297 (comment + check):
```
            None => {
                // `jobTemplateParameters` only has meaning as the parameters OF a job
                // template, so parameters without one are refused rather than quietly
                // dropped: there is nothing for them to be substituted into.
                if input.job_template_parameters.is_some() {
                    return Err(StartJobRunErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "jobTemplateParameters requires jobTemplateId to be specified"
                                    .to_string(),
                            ),
                        },
                    )
                    .into());
                }
```
The rule it breaks — services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/state.rs:1179-1181 (`RequestRendering` doc, the list of what makes two renderings equal):
```
///   * a collection the request omitted and one it sent EMPTY render alike, because a
///     handler persists the same record for both. A structure is not collapsed that way: an
///     empty one is a member the request carried.
```
and src/state.rs:1233-1235 (`RequestRendering::container`):
```
    /// `collapse_empty` is what makes an absent collection and an empty one render alike,
    /// and it applies only where the container sits in a structure MEMBER: that is the
    /// only position a request can leave unwritten.
```

Siblings in the SAME crate that do normalise empty -> absent:
- src/state.rs:569-578 — doc: "A filter the request omitted constrains nothing, and nor does one that lists no state at all — an empty list names no state to keep out." and the code `states.filter(|states| !states.is_empty()).is_none_or(..)`.
- src/ops/list_managed_endpoints.rs:92 — `if let Some(types) = input.types.as_ref().filter(|types| !types.is_empty()) {`.
- src/state.rs:208-215 `configured_scheduler_limits` — doc: "`None` when it configures none — both an absent structure and one whose every member is omitted, which are the same statement".
- src/ops/start_job_run.rs:337 — `let mut tags = input.tags.clone().unwrap_or_default();` (absent tags and empty tags persist the same record).

Why the two bodies are one request by this crate's definition — tests/all/spent_token_with_a_different_request.rs:16-19:
```
//! because answering it with the earlier resource would silently drop the work this call
//! asked for. Two spellings of one request (a collection omitted beside the same
//! collection sent empty, a map whose entries are stored in hash order) are the same
//! request and still replay.
```

Test coverage: the existing refusal is pinned only for a NON-empty map — tests/all/virtual_cluster_job_run_loop.rs:856-865 sends `json!({"clientToken": "job-template-params", "jobTemplateParameters": {"rows": "10"}})` and expects 400 `ValidationException` ("job template parameters with no template"). No test sends an EMPTY map, so treating `{}` as absent leaves every existing test unchanged. The model admits the value: `StartJobRunRequest$jobTemplateParameters` is `map<string,string>, len 0..100` (OPERATIONS.md:181).

## Steps to Reproduce

1. POST /virtualclusters with a valid `CreateVirtualCluster` body and note the returned cluster `id`.
2. POST /virtualclusters/{id}/jobruns with body {"clientToken":"t1","executionRoleArn":"arn:aws:iam::123456789012:role/job","releaseLabel":"emr-6.10.0-latest","jobDriver":{"sparkSubmitJobDriver":{"entryPoint":"s3://bucket/job.py"}},"jobTemplateParameters":{}} — observe HTTP 400 ValidationException "jobTemplateParameters requires jobTemplateId to be specified" (src/ops/start_job_run.rs:287).
3. POST the same body with the `jobTemplateParameters` member omitted entirely — observe HTTP 200 and a job run created.
4. Note that src/state.rs:1179-1181 defines those two bodies as the SAME request (they produce an identical `request_fingerprint`), so one request receives two different answers depending only on whether the client serialised the empty map; and that an empty map supplies no parameter for the handler's own stated reason ("would be silently dropped") to apply.
