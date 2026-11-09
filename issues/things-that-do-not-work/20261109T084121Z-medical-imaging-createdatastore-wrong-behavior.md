Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b046b5567c97d3bc5808b738f77055e3138493fc

# The signing region reaches datastore_arn and the import client's endpoint authority unnarrowed, against the crate's own claim that request_region narrows it

_Filed: 2026-11-09T08:41:21Z_
_Operation: CreateDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b046b5567c97d3bc5808b738f77055e3138493fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::request_region` returns the SigV4 credential scope's region field verbatim, with no check on its shape. The crate, however, states in writing that it does narrow it: `src/state/dicom_import.rs` tells the reader that the signing region is "narrowed before it gets here" and that `request_region` "admits only a value shaped like a Region identifier and otherwise answers the default Region". No such narrowing exists anywhere in the crate (grep for `request_region` finds only the 18 handler call sites and the one definition), and the framework parser it delegates to does not do it either: `sigv4_credential_scope` accepts any non-empty field between two `/` separators, so uppercase letters, dots, spaces and `:` all survive.

Two concrete consequences of the missing narrowing:

1. `CreateDatastore` mints the data store's ARN from that raw text (`crate::state::datastore_arn(&region, &datastore_id)`), and `emulator_arn` interpolates it verbatim into `arn:aws:medical-imaging:{region}:000000000000:datastore/{id}`. The model constrains the region segment of the `Arn` shape to `[a-z0-9-]+`, so a credential scope whose region field carries anything else makes `GetDatastore`/`ListDatastores` report a `datastoreArn` that violates the `@pattern` the model declares on the very member carrying it — and that this crate's own `parse_resource_arn` then refuses. The service therefore hands a client an ARN its own tagging operations answer `BadRequestException` for: `TagResource`/`UntagResource`/`ListTagsForResource` on the ARN `GetDatastore` just reported are rejected as malformed. `CopyImageSet`'s minted `imageSetArn` inherits the same defect, since `image_set_arn` suffixes the data store's ARN.

2. The import job's Amazon S3 client builds its authority out of the same text — `endpoint_url(format!("https://s3.{region}.amazonaws.com"))` — which is precisely the field the quoted comment claims is protected by a narrowing that does not exist.

The defect is the absent narrowing (and the comment asserting it), not any constant: either `request_region` must apply the `[a-z0-9-]+` shape the crate's own `ARN_PATTERN` and `parse_resource_arn` already enforce on the region segment and fall back to `DEFAULT_REGION` otherwise, as the comment says it does, or the comment is false.

## Evidence

The crate's own claim — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state/dicom_import.rs:410-413`:
```
/// this client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or prefix
/// a caller names. The one caller-supplied value that reaches its authority is the signing
/// region, and it is narrowed before it gets here — [`super::request_region`] admits only
/// a value shaped like a Region identifier and otherwise answers the default Region.
```

The code it claims that of — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:2208-2212`:
```rust
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}
```
No shape test at all: the scope's `region` is returned as-is. The parser it delegates to applies none either — `vendor/server/src/lib.rs:1946`: `if access_key_id.is_empty() || date.is_empty() || region.is_empty() || service.is_empty() { return None; }` is the whole of the validation.

Where the unnarrowed value lands — `services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/ops/create_datastore.rs:238`:
```rust
datastore_arn: crate::state::datastore_arn(&region, &datastore_id),
```
and `state.rs:2216-2218`:
```rust
pub fn datastore_arn(region: &str, datastore_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_SERVICE, region, &format!("datastore/{datastore_id}"))
}
```
with `vendor/server/src/lib.rs:170-172`: `format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")` — verbatim interpolation.

The model's constraint the emitted value can break — `smithy_query(query="shape",shape="DatastoreProperties")`, member `datastoreArn`:
```
"datastoreArn": {"target": "Arn", "pattern": "^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}(/imageset/[0-9a-z]{32})?$"}
```
The crate restates the same rule for its parser — `state.rs:2284-2292`:
```rust
// `[a-z0-9-]+`: the pattern constrains the region's alphabet, not its shape.
let region = fields.next()?;
if region.is_empty()
    || !region.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
{
    return None;
}
```
so `parse_resource_arn` refuses exactly the ARNs `datastore_arn` can mint, and `src/ops/tag_resource.rs:171-175` turns that into `BadRequestException(MALFORMED_ARN_MESSAGE)`.

The S3 authority — `src/state/dicom_import.rs:425-426`:
```rust
.region(::aws_sdk_s3::config::Region::new(region.to_string()))
.endpoint_url(format!("https://s3.{region}.amazonaws.com"))
```

Test coverage: no test drives a region outside the alphabet. `tests/all/regional_isolation.rs:31-32` says only "Real AWS region codes, every one of them inside the `[a-z0-9-]+` alphabet the service's own ARN pattern admits for a region" — i.e. the suite deliberately stays inside the alphabet and states no reason for the handler to accept values outside it, so nothing pins the current behaviour.

## Steps to Reproduce

1. Send `POST /datastore` with body `{"clientToken":"t1","datastoreName":"store.one"}` and header `Authorization: AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1:X/medical-imaging/aws4_request, SignedHeaders=host, Signature=...` — `sigv4_credential_scope` parses the region field as `us-east-1:X` and `request_region` returns it unchanged, instead of answering `DEFAULT_REGION` as `src/state/dicom_import.rs:412-413` says it does.
2. Wait out `DATASTORE_ACTIVATION_DELAY`, then `GET /datastore/{datastoreId}` signed the same way: the response reports `datastoreProperties.datastoreArn = arn:aws:medical-imaging:us-east-1:X:000000000000:datastore/<id>`, which does not match the `@pattern` the model declares on that member (`...:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/...`).
3. Feed that reported ARN straight back to the service: `POST /tags/{url-encoded datastoreArn}` with `{"tags":{"k":"v"}}`. The request is refused with HTTP 400 `BadRequestException` / `Invalid input resource arn: bad syntax` (src/service.rs:157-162 and src/ops/tag_resource.rs:171-175), because `parse_resource_arn` rejects the region alphabet — so the service cannot tag a resource using the ARN it itself reported.
4. Also observe `POST /startDICOMImportJob/datastore/{datastoreId}` under the same credential: `s3_client` is built with `endpoint_url("https://s3.us-east-1:X.amazonaws.com")`, i.e. the caller-supplied text reached the client's authority — the one value the quoted comment asserts is narrowed first.
