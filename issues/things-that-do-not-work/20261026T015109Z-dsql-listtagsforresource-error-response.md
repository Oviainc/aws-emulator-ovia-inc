Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# A nested stream ARN that names no stream answers ResourceNotFoundException with resourceType "cluster" and the composite resourceId

_Filed: 2026-10-26T01:51:09Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging operations resolve a `resourceArn` through `parse_resource_arn`, which — as the crate documents — splits the resource field at its FIRST separator, so a stream's nested ARN (`...:cluster/{cluster}/stream/{stream}`) arrives as resource_type `cluster` with resource_id `{cluster}/stream/{stream}`. When that ARN names no stream this store holds, the handler feeds those two parsed fields straight into the modelled `ResourceNotFoundException`, so the 404 for a MISSING STREAM reports `resourceType: "cluster"` and `resourceId: "{cluster}/stream/{stream}"`. The crate declares elsewhere that a stream's `ResourceNotFoundException` carries resourceType `stream`, and `GetStream`'s 404 for the very same missing stream does exactly that (resourceType `stream`, resourceId the 26-character stream id alone). Two paths of this crate therefore describe the same absent resource with different `resourceType` and `resourceId` values, and the tagging one names the wrong resource kind. `TagResource` and `UntagResource` share the same resolution and the same refusal, so all three answer this way.

## Evidence

The declared rule, `src/state.rs:92-95`:
```
/// The resource-type segment of a stream ARN
/// (`...:cluster/{cluster}/stream/{stream}`), and the `resourceType` a stream's
/// `ResourceNotFoundException` carries.
pub const RESOURCE_TYPE_STREAM: &str = "stream";
```
The parse the tagging path performs, `src/state.rs:673-678`:
```
    /// Both tagged resources of this service are written under the cluster's own
    /// resource type: a cluster is `cluster/{identifier}` and a stream of it is
    /// `cluster/{cluster}/stream/{stream}` (`ClusterArn` / `StreamArn`), and
    /// [`parse_resource_arn`] splits the resource field at its FIRST separator, so
    /// a stream arrives as the type `cluster` with the id `{cluster}/stream/{stream}`.
```
The refusal built from those parsed fields, `src/ops/list_tags_for_resource.rs:179-191`:
```
        let Some(tags) = state
            .resource_tags(&resource, &crate::state::request_region(ctx))
            .cloned()
        else {
            return Err(ListTagsForResourceErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(
                    &input.resource_arn,
                    &resource.resource_id,
                    &resource.resource_type,
                ),
            )
            .into());
        };
```
(identical code in `src/ops/tag_resource.rs:231-241` and `src/ops/untag_resource.rs:216-226`).
The other side — what the crate reports for the same absent stream, `src/state.rs:1371-1382`:
```
/// The modelled `ResourceNotFoundException` for a stream a cluster does not hold.
pub fn stream_not_found(
    region: &str,
    cluster_identifier: &str,
    stream_identifier: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        &stream_arn(region, cluster_identifier, stream_identifier),
        stream_identifier,
        RESOURCE_TYPE_STREAM,
    )
}
```
used by `src/ops/get_stream.rs:177-186`.
Existing coverage pins only a TOP-LEVEL `stream/...` ARN, never the nested form, `tests/all/resource_tags.rs:305-313`:
```
        (
            "another Aurora DSQL resource type this store holds none of",
            format!(
                "arn:aws:dsql:us-east-1:{}:stream/abcdefghijklmnopqrstuvwxyz",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "stream",
```
so no test pins the nested-stream answer.

## Steps to Reproduce

1. POST /cluster with {"deletionProtectionEnabled": false}, wait for the cluster to settle to ACTIVE, and note {cluster}
2. GET /tags/arn%3Aaws%3Adsql%3Aus-east-1%3A{account}%3Acluster%2F{cluster}%2Fstream%2Fzzzzzzzzzzzzzzzzzzzzzzzzzz (a well-formed stream ARN under the live cluster, naming a stream that was never created)
3. Observe 404 ResourceNotFoundException whose resourceType is "cluster" and whose resourceId is "{cluster}/stream/zzzzzzzzzzzzzzzzzzzzzzzzzz"
4. GET /stream/{cluster}/zzzzzzzzzzzzzzzzzzzzzzzzzz for the same missing stream — 404 with resourceType "stream" and resourceId "zzzzzzzzzzzzzzzzzzzzzzzzzz"
5. Repeat step 2 with POST /tags/{arn} and DELETE /tags/{arn}?tagKeys=env: the same mismatched members
