Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4c075dcdeacefbec5f942454c2320d1c6eabf26d

# GetImageSetMetadata reports contentType application/json, contradicting the documented default its own member declares

_Filed: 2026-10-29T00:50:43Z_
_Operation: GetImageSetMetadata | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4c075dcdeacefbec5f942454c2320d1c6eabf26d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetImageSetMetadataResponse.contentType` is documented by the model as "The format in which the study metadata is returned to the customer. Default is `text/plain`." The handler never reports that format: it hardcodes a crate constant `METADATA_CONTENT_TYPE = "application/json"` and puts it on every successful answer, so the `Content-Type` header a client sees for the aggregated metadata document is never the one the member documents as the format this operation returns the document in.

The crate treats the SAME kind of model sentence as binding one operation over: `GetImageFrame`'s `contentType` member documents "Default is `application/octet-stream`", and this crate transcribes that sentence into `crate::state::DEFAULT_IMAGE_FRAME_CONTENT_TYPE` and serves it (plus the whole per-transfer-syntax table the same member documents). So one of the two content reads implements its member's documented content type and the sibling reads it, states a different value, and documents the divergence only as a preference ("the document in the blob is JSON") — with no model text, comment or test reason that reconciles it with the sentence the member actually carries.

The defect is the header VALUE only: the blob itself is the JSON document this crate builds, and nothing about how the document is serialised is in question.

## Evidence

Model (authority for the value):
`smithy_query(query="doc", shape="GetImageSetMetadataResponse", member="contentType")` →
  "<p>The format in which the study metadata is returned to the customer. Default is <code>text/plain</code>.</p>"

Handler (what it actually answers):
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/get_image_set_metadata.rs:108-109
  `/// The `contentType` this operation reports: the document in the blob is JSON.`
  `pub const METADATA_CONTENT_TYPE: &str = "application/json";`
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/get_image_set_metadata.rs:453-455
  `let output = GetImageSetMetadataOutput {`
  `    image_set_metadata_blob: serialized,`
  `    content_type: Some(METADATA_CONTENT_TYPE.to_string()),`

The same crate applying the opposite rule to the sibling content read:
services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:861-864
  `/// The `contentType` `GetImageFrame` reports for pixel data whose stored transfer`
  `/// syntax the response documentation does not list — the documented default`
  `/// ("Default is `application/octet-stream`").`
  `pub const DEFAULT_IMAGE_FRAME_CONTENT_TYPE: &str = "application/octet-stream";`
(and state.rs:874-877, which transcribes the rest of that member's documented table verbatim)

Test coverage that pins the current value (both would move with the fix):
services/service-com-amazonaws-medicalimaging-ahigatewayservice/tests/all/image_set_metadata.rs:57-58
  `/// The `contentType` the document is served with: the blob holds JSON.`
  `const EXPECTED_CONTENT_TYPE: &str = "application/json";`
  …asserted at tests/all/image_set_metadata.rs:100-104
services/service-com-amazonaws-medicalimaging-ahigatewayservice/tests/all/image_set_metadata_updates.rs:67-68 and :197-199
  `/// The `contentType` `GetImageSetMetadata` serves the document with.`
  `const METADATA_CONTENT_TYPE: &str = "application/json";`

## Steps to Reproduce

1. `POST /datastore` with a `clientToken` to create a data store, and wait out `DATASTORE_ACTIVATION_DELAY` so it reports ACTIVE.
2. Get an image set into the store (the suites seed one through the crate's `state` module, as `tests/all/image_set_metadata.rs` does).
3. `POST /datastore/{datastoreId}/imageSet/{imageSetId}/getImageSetMetadata`.
4. Read the response's `Content-Type` header: it is `application/json`.
5. Compare with the member's own documentation — "The format in which the study metadata is returned to the customer. Default is `text/plain`." — and with `crate::state::DEFAULT_IMAGE_FRAME_CONTENT_TYPE`, where this crate does serve the value the equivalent sentence documents for `GetImageFrame`.
