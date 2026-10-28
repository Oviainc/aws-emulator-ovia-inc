Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 8668e0a390f031d37f864cf13043544a41d29fed

# DeleteDatastore's "still contains image sets" conflict interpolates the data store identifier, unlike every other refusal text this service words

_Filed: 2026-10-28T15:03:55Z_
_Operation: DeleteDatastore | Class: error-response_

> _Note: this issue was identified against the code base at commit `8668e0a390f031d37f864cf13043544a41d29fed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal text in this crate is a constant of the state module that carries no identifier from the request — including the two other refusals `DeleteDatastore` itself raises (`DATASTORE_NOT_FOUND_MESSAGE` for the 404 and `DATASTORE_STILL_CREATING_MESSAGE` for the CREATING conflict). The image-sets-remaining conflict is the single exception: it is built inline with `format!` and interpolates `datastore.datastore_id`, so one of this operation's three refusals reflects a request identifier back while the other two state the rule only. The text also lives in the handler rather than beside the service's other refusal constants, so it is the one refusal a reader of `state.rs` cannot find and the one a second writer of the same rule would have to respell.

## Evidence

The inline, identifier-interpolating refusal — `src/ops/delete_datastore.rs:189-197`:
```
        if held_image_sets > 0 {
            return Err(DeleteDatastoreErrorModeled::ConflictException(ConflictException {
                message: format!(
                    "Datastore {} still contains image sets; delete them before deleting the data store",
                    datastore.datastore_id
                ),
            })
            .into());
        }
```
The same handler's other two refusals, both state-module constants with no identifier in them — `src/ops/delete_datastore.rs:160-164` and `:172-177`:
```
                DeleteDatastoreErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    // Constant, and stated by resource KIND: the requested id is not
                    // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                    message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
```
```
        if datastore.creation_in_flight() {
            return Err(DeleteDatastoreErrorModeled::ConflictException(ConflictException {
                message: crate::state::DATASTORE_STILL_CREATING_MESSAGE.to_string(),
            })
```
The rule the state module states for this service's refusal texts — `src/state.rs:205-216` and the conflict constants it keeps, e.g. `src/state.rs:320-332`:
```
// Three wording rules live here because more than one handler — and the
// `override_refusal` seam in `crate::service` — states them, and a text spelled
// twice drifts:
//
// 1. A missing resource is stated by its KIND, as a constant: the text carries no
//    identifier from the request, so one caller probing ids cannot read them back
//    out of the 404 body
```
```
/// The text this service refuses a call that would act on a data store, or on the
/// contents of one, that has not settled into `ACTIVE` with — one still `CREATING` or
/// already `DELETING`. Constant, and stated by rule rather than by which store it was,
/// exactly like [`IMAGE_SET_NOT_ACTIVE_MESSAGE`] one resource kind down.
pub const DATASTORE_NOT_ACTIVE_MESSAGE: &str =
    "The data store is not in the ACTIVE state, so it cannot serve this request.";
```
Even the one refusal text that does carry a number from the request documents the limit explicitly — `src/state.rs:539-549`: "a count computed from the request, so the sentence says which side of the bound the caller landed on without echoing any key". No test pins the image-sets-remaining text: grepping the crate for `still contains image sets` matches only `src/ops/delete_datastore.rs:192`.

## Steps to Reproduce

1. Create a data store, let it settle to ACTIVE, and get one live (not DELETED) image set into it.
2. `DELETE /datastore/{datastoreId}`.
3. Expected, by the wording this service applies to its other refusals (state.rs:205-216, state.rs:320-325, and the two sibling refusals in this same handler at delete_datastore.rs:160-164 and :172-177): a constant sentence stating the rule, carrying no identifier from the request.
4. Actual: HTTP 409 with `"Datastore <the 32-character datastoreId from the request URI> still contains image sets; delete them before deleting the data store"`, the only refusal text in the crate that interpolates a resource identifier.
