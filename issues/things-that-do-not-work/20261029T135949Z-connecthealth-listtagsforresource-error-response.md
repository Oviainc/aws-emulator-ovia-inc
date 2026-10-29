Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 3c3c87ac9166712e1ad44c6d25ef542a1ff1b247

# The ARN-not-found refusal echoes the whole unbounded resourceArn, the "one request cannot size the response it is answered with" rule every other refusal of this crate keeps

_Filed: 2026-10-29T13:59:49Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `3c3c87ac9166712e1ad44c6d25ef542a1ff1b247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`resourceArn` is the only member of this service that a handler reads and echoes which the model bounds in no way at all: `smithy_query(query="operation", operation="ListTagsForResource")` reports it as `{"target": "String", "httpLabel": true, "required": true}` — no `@length`, no `@pattern`. `crate::service::error_detail::resource_not_found_message` interpolates that value verbatim into the `ResourceNotFoundException` detail text, so the size of the refusal body is chosen by the caller.

Every OTHER value this crate reads off a request and either stores or reports is held to a ceiling for exactly that reason, and the crate says so each time: the pagination cursor (`MAX_PAGE_TOKEN_LEN` — "the decode allocation would otherwise be sized by what a caller sends"), the credential-scope region (`MAX_REGION_LEN` — "the bound exists because the field is read off the request"), the Identity Center instance id carried into a reported application id (`MAX_IDC_INSTANCE_SEGMENT`), and — most directly — the tag refusal built in `tag_bounds`, which names at most `MAX_NAMED_ENTRIES` entries precisely "so one request cannot size the response it is answered with" and renders each clause through the framework walk, which elides an over-long value. Every other not-found sentence in `error_detail.rs` echoes a MODEL-BOUNDED identifier (`domainId` 20..25, `subscriptionId` 25..25, `sessionId` 36..36, `jobId` 1..36); the ARN sentence is the one that echoes an unbounded one.

The defect is the inconsistency, not the number: one refusal path of this crate lets the request decide how large the answer is, while the sibling refusal paths of the same crate (including the tag refusal that the very same tagging handlers raise before this one) deliberately do not.

## Evidence

The rule, stated in this crate — `services/service-com-amazonaws-connecthealth-connecthealth/src/service/tag_bounds.rs:62-63`:
```
//! `TagKeyList` arrives already ordered by the request, and its elements keep that order.
//! A refusal names at most [`MAX_NAMED_ENTRIES`] entries, so one request cannot size the
//! response it is answered with.
```

The same policy, twice more, for values read off the request — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1464-1470`:
```
/// The longest `nextToken` this service attempts to decode at all.
...
/// modelled id — well inside this bound. The limit exists because the decode allocation
/// would otherwise be sized by what a caller sends.
const MAX_PAGE_TOKEN_LEN: usize = 512;
```
and `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:1400-1406`:
```
/// The longest credential-scope region field this service files a record under.
///
/// Every region code AWS publishes is far shorter; the bound exists because the field is
/// read off the request, so without it one request could file a record under a key of
/// arbitrary size.
const MAX_REGION_LEN: usize = 32;
```

The path that breaks it — `services/service-com-amazonaws-connecthealth-connecthealth/src/service/error_detail.rs:183-191`:
```
/// The detail text of the `ResourceNotFoundException` answered when the ARN a tagging
/// call names resolves to no resource this service holds.
///
/// The same `<resource kind> not found: <identifier>` form as the two constructors
/// above, with the identifier the request used — here a resource ARN, since that is
/// what the tagging operations address a resource by.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource not found: {resource_arn}")
}
```

The caller that reaches it with the raw request value — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/list_tags_for_resource.rs:185-194`:
```
        if !store.holds_resource_arn(&region, &input.resource_arn) {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::service::error_detail::resource_not_found_message(
                        &input.resource_arn,
                    )),
                },
            )
            .into());
        }
```
(the same two lines appear at `tag_resource.rs:234-243` and `untag_resource.rs:226-235`, and `tag_resource.rs:255-265` interpolates `input.resource_arn` into its quota sentence as well).

Proof the echoed member carries no modelled bound — `smithy_query(query="operation", operation="ListTagsForResource")`:
```
"input": {"resourceArn": {"target": "String", "httpLabel": true, "required": true}}
```
while the identifiers every other sentence in the same module echoes are bounded: `domain_not_found_message` (`DomainId`, len 20..25), `subscription_not_found_message` (20..25 and 25..25), `scribe_session_not_found_message` (36..36), `patient_insights_job_not_found_message` (1..36).

Test coverage of the paths involved: `tests/all/list_tags_for_resource.rs`, `tests/all/resource_tagging.rs`, `tests/all/resource_tag_removal.rs` and `tests/all/tag_entry_refusal_completeness.rs` assert the status, the error code and the sentence for ARNs of ordinary length, so none of them pins the behaviour for a caller-sized ARN.

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/service/tag_bounds.rs:62-63 — the crate states that a refusal must not let one request size the response it is answered with — and state.rs:1400-1406 / :1464-1470, where the same reasoning bounds the two other values read off a request.
2. Read src/service/error_detail.rs:183-191: the ARN not-found sentence interpolates `resource_arn` verbatim, with no ceiling and no elision.
3. Confirm with smithy_query(query="operation", operation="ListTagsForResource") that `resourceArn` is a bare `String` with no @length, unlike every other identifier echoed by that module (domainId 20..25, subscriptionId 25..25, sessionId 36..36, jobId 1..36).
4. Send GET /tags/{resourceArn} with a long percent-encoded resourceArn that names no resource and observe the 404 body carrying the whole value back; compare with a request whose tags map is unacceptable, which is refused through tag_bounds and names at most MAX_NAMED_ENTRIES entries with over-long values elided.
