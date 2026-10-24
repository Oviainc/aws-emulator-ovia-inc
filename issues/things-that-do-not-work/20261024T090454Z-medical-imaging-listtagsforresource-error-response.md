Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Tagging operations word ResourceNotFoundException by echoing the caller's ARN instead of the resource kind

_Filed: 2026-10-24T09:04:54Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other operation in this crate words a missing resource with this service's constant, resource-KIND text — `"Datastore resource is not found."` / `"ImageSet resource is not found."` / `"ImageFrame resource is not found."` — and `src/state.rs` states that rule explicitly ("A missing resource is stated by its KIND, as a constant: the text carries no identifier from the request"). The three tagging handlers break it: they build the 404 message with `format!("Resource {} was not found", input.resource_arn)`, so the body of a ListTagsForResource/TagResource/UntagResource 404 reads "Resource arn:aws:medical-imaging:us-east-1:000000000000:datastore/<id> was not found" instead of the service's own sentence for a data store or image set that is not there.

Expected: a tagging call naming a data store the service does not hold answers `ResourceNotFoundException` with the same resource-kind sentence the rest of the service uses (and the image-set sentence when the ARN carries the `/imageset/<id>` suffix), with no request-supplied identifier interpolated. Actual: the whole caller-supplied ARN is reflected back in the message, and the text differs per route from every other 404 the service emits.

## Evidence

Service error-text contract: `src/state.rs` ("How this service WORDS a refusal", rule 1) plus the constants `DATASTORE_NOT_FOUND_MESSAGE` / `IMAGE_SET_NOT_FOUND_MESSAGE` used by `get_datastore.rs`, `delete_datastore.rs`, `get_image_set.rs`, `get_image_frame.rs`, `copy_image_set.rs`, `list_dicom_import_jobs.rs`. Divergent handlers: `src/ops/list_tags_for_resource.rs` (`message: format!("Resource {} was not found", input.resource_arn)`), `src/ops/tag_resource.rs` and `src/ops/untag_resource.rs` (identical format). Test coverage: `tests/all/resource_tagging.rs` asserts only HTTP 404 and `error_code == "ResourceNotFoundException"` on the tagging routes (lines ~296-355) — the message text is unasserted, and `tests/all/error_message_texts.rs` only pins `DATASTORE_NOT_FOUND` for the non-tagging routes.

## Steps to Reproduce

1. Start the emulator and pick a well-formed data store ARN for an id no call ever minted, e.g. arn:aws:medical-imaging:us-east-1:000000000000:datastore/00000000000000000000000000000000.
2. GET /tags/{urlEncodedArn} (ListTagsForResource).
3. Observe HTTP 404 ResourceNotFoundException whose message echoes the submitted ARN, while GET /datastore/00000000000000000000000000000000 answers the same missing data store with "Datastore resource is not found."
4. Repeat with POST /tags/{arn} (TagResource) and DELETE /tags/{arn}?tagKeys=k (UntagResource) — both reflect the ARN back the same way.
