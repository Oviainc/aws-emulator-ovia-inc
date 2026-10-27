Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# CreateStream's ConflictException and ServiceQuotaExceededException report resourceType "stream" with the CLUSTER's identifier as resourceId

_Filed: 2026-10-27T23:08:01Z_
_Operation: CreateStream | Class: error-response_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both of `CreateStream`'s own refusals name a resource by a contradictory pair: `resourceType` says `stream` while `resourceId` carries the CLUSTER identifier the request addressed.

* the `clientToken`-mismatch `ConflictException` sets `resource_id: Some(input.cluster_identifier.clone())` and `resource_type: Some("stream".to_string())`;
* the quota refusal sets `resource_id: cluster.identifier.clone()` and `resource_type: "stream".to_string()` (with `quota_code: "streams-per-cluster"`).

The crate has exactly one rule for the pair, stated in `state.rs` and applied everywhere else: a resource is identified by the type segment it is stored under together with that resource's OWN identifier — a stream by `RESOURCE_TYPE_STREAM` plus its stream identifier, a cluster by `RESOURCE_TYPE_CLUSTER` plus its cluster identifier. `DeleteStream`'s token-mismatch conflict, `Stream::mutation_conflict`, `Cluster::mutation_conflict`, `stream_not_found` and `TagResource`'s quota refusal (which reads the pair out of `addressed_identity`) all follow it; these two refusals do not. A client that reads the 409/402 body is told a STREAM with that id is at fault, and the id it is handed names a cluster — the same class of defect as the already-fixed nested-ARN identity issue (`20261026T015109Z-dsql-listtagsforresource-error-response`), reintroduced on the create path.

## Evidence

`services/service-com-amazonaws-dsql-dsql/src/ops/create_stream.rs:286-292` (token mismatch):
```
crate::state::TokenVerdict::Mismatch => {
    return Err(CreateStreamErrorModeled::ConflictException(ConflictException {
        message: crate::state::CLIENT_TOKEN_MISMATCH_MESSAGE.to_string(),
        resource_id: Some(input.cluster_identifier.clone()),
        resource_type: Some("stream".to_string()),
```
`src/ops/create_stream.rs:314-325` (quota):
```
if cluster.streams.len() >= crate::state::MAX_STREAMS_PER_CLUSTER {
    return Err(CreateStreamErrorModeled::ServiceQuotaExceededException(
        ServiceQuotaExceededException {
            ...
            resource_id: cluster.identifier.clone(),
            resource_type: "stream".to_string(),
```
The rule the crate states, `src/state.rs:975-983`:
```
/// The identity a modelled error reports for the resource a tagging ARN names:
/// that resource's OWN type segment and its OWN identifier.
///
/// A resource is identified by the type segment it is stored under and the
/// identifier that segment carries, so a stream of a cluster is a
/// [`RESOURCE_TYPE_STREAM`] named by its own stream identifier
```
The sibling that follows it, `src/ops/delete_stream.rs:221-226`:
```
crate::state::TokenVerdict::Mismatch => {
    return Err(DeleteStreamErrorModeled::ConflictException(ConflictException {
        message: crate::state::CLIENT_TOKEN_MISMATCH_MESSAGE.to_string(),
        resource_id: Some(input.stream_identifier.clone()),
        resource_type: Some(crate::state::RESOURCE_TYPE_STREAM.to_string()),
```
and `src/state.rs:214-221` (`Stream::mutation_conflict`: `resource_id: Some(self.stream_identifier.clone())`, `resource_type: Some(RESOURCE_TYPE_STREAM...)`), plus `src/ops/tag_resource.rs:262-263` (`resource_id: identity.resource_id.clone(), resource_type: identity.resource_type.clone()`).

The model treats the two members as describing ONE resource — `smithy_query(query="shape", shape="ServiceQuotaExceededException")`: `resourceId` (required) "The resource ID exceeds a quota", `resourceType` (required) "The resource type exceeds a quota".

Test coverage: `tests/all/stream_lifecycle.rs` and `tests/all/stream_deletion.rs` assert the status/arn of stream calls; `tests/all/error_contract.rs` and `tests/all/nested_arn_not_found.rs` pin resourceId/resourceType for not-found bodies, but no test asserts the identity pair of CreateStream's 409 or 402.

## Steps to Reproduce

1. POST /cluster {"deletionProtectionEnabled": false}; wait for the cluster to settle to ACTIVE; note the identifier K.
2. POST /stream/K with {"targetDefinition": {"kinesis": {"streamArn": "arn:aws:kinesis:us-east-1:<account>:stream/changes", "roleArn": "arn:aws:iam::<account>:role/dsql-cdc"}}, "ordering": "UNORDERED", "format": "JSON", "clientToken": "tok-1"} -> 200.
3. POST /stream/K again with the same clientToken "tok-1" but a different targetDefinition (another Kinesis streamArn) -> 409 ConflictException whose body carries resourceType "stream" and resourceId = K, the CLUSTER's identifier.
4. Compare with DELETE /stream/K/{streamIdentifier} reused under a token for another stream, whose 409 carries resourceType "stream" with the stream's own identifier.
5. Create MAX_STREAMS_PER_CLUSTER streams on K, then one more -> 402 ServiceQuotaExceededException whose resourceType is "stream" while resourceId is again the cluster identifier K.
