Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# MediaStreamTags are stored unchecked: the tag-pair rule every other tag member of this crate is written under is skipped for media streams

_Filed: 2026-10-29T01:13:00Z_
_Operation: AddFlowMediaStreams | Class: validation_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AddMediaStreamRequest$MediaStreamTags` is persisted verbatim on the stored media stream record, but neither AddFlowMediaStreams nor the CreateFlow `MediaStreams` path runs it through `crate::state::tag_set_rejection` / `tag_pair_rejection` — the one rule every other tag member of this crate is written under. The consequence is that a media stream can be stored with a tag pair the crate refuses everywhere else: an EMPTY tag key (which `tag_pair_rejection` calls "A tag key must not be empty."), an over-long key or value, and an unbounded number of pairs — in a record nothing evicts.

This is a sibling asymmetry inside this crate, not a new rule: AddFlowOutputs checks `OutputTags`, AddFlowSources checks `SourceTags`, GrantFlowEntitlements checks `EntitlementTags`, CreateFlow checks `FlowTags` plus all three sub-resource maps, and each of the three router creates checks its own `Tags` — all through the same `crate::state` functions. Only the media-stream path writes a tag map with no check at all, although `state.rs` names `MediaStreamTags` as one of the create tag members and states that a create's tag member "is bounded by the same two rules" as a tag write.

## Evidence

The crate's single rule for a create request's tag member, which names the two checks every such member gets — src/state.rs:824-849:
```
/// Why the tag member `tags` of a create request is not a tag map this service stores on
/// `resource`, or `None` when it is.
///
/// Every tag member of a create request writes the same kind of tag map TagResource writes
/// on the same resource, so it is bounded by the same two rules, read from the same place:
/// each pair through [`tag_pair_rejection`], and the count through
/// [`MAX_TAGS_PER_RESOURCE`] - a create cannot leave a resource carrying more tags than a
/// tag write could put on it.
pub fn tag_set_rejection(
```
and the pair rule it reads, whose first clause is not a cap at all — src/state.rs:730-733:
```
pub fn tag_pair_rejection(key: &str, value: &str) -> Option<String> {
    if key.is_empty() {
        return Some("A tag key must not be empty.".to_string());
    }
```
`MediaStreamTags` is explicitly one of the create tag members — src/state.rs:811-815:
```
/// The tag map a resource is created with, from the tag member of the request that created
/// it (`FlowTags`, `OutputTags`, `SourceTags`, `EntitlementTags`, `MediaStreamTags`): the
/// pairs the member carries, ordered, and EMPTY when the member is absent
```
and it IS stored verbatim — src/state.rs:2096-2101:
```
        tags: request
            .media_stream_tags
            .iter()
            .flatten()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
```
The handler checks the media stream's name/id and then stores it, with no tag check anywhere in the file — src/ops/add_flow_media_streams.rs:235-249:
```
        for request in &input.media_streams {
            if let Some(rejection) = crate::state::media_stream_rejection(request, &names, &ids) {
                return Err(bad_request(rejection));
            }
            names.insert(request.media_stream_name.clone());
            ids.insert(request.media_stream_id);
        }
...
        for request in &input.media_streams {
            let fmt = crate::state::next_media_stream_fmt(&flow.media_streams);
            flow.media_streams
                .push(crate::state::materialise_media_stream(request, fmt));
        }
```
The siblings that DO apply it — src/ops/add_flow_outputs.rs:263-269:
```
        for (index, output) in input.outputs.iter().enumerate() {
            if let Some(rejection) = crate::state::tag_set_rejection(
                &format!("Outputs[{index}]"),
                output.output_tags.as_ref(),
            ) {
                return Err(bad_request(rejection));
            }
```
and src/ops/create_flow.rs:328-349, which enumerates the sub-resource tag members it checks and omits the media-stream one:
```
        // Each sub-resource this request can create carries a tag member of its OWN
        // (`SourceTags`, `OutputTags`, `EntitlementTags`), and each of those sub-resources
        // has its own ARN ... and each
        // tag map is bounded by the same rules `FlowTags` is (see
        // `crate::state::tag_set_rejection`).
```
The crate's own stated reason for bounding a stored tag map — src/ops/create_router_input.rs:250-252:
```
        // Tags are stored verbatim in a record nothing evicts, so the pairs and their count
        // are bounded before they are stored, through the same functions every other tag
        // write in this crate uses.
```
Test coverage: tests/all/flow_media_streams.rs:287-317 (`add_flow_media_streams_stores_the_tags_the_request_carried`) pins only that a VALID tag map is stored and not reported on the `MediaStream` shape; no test states that an empty-keyed or unbounded media-stream tag map is acceptable.

## Steps to Reproduce

1. Create a flow: POST /v1/flows with {"name":"ms-tags","source":{"name":"s1","protocol":"rtp"}} and note the FlowArn.
2. POST /v1/flows/{FlowArn}/mediaStreams with {"mediaStreams":[{"mediaStreamId":1,"mediaStreamName":"tagged","mediaStreamType":"video","mediaStreamTags":{"":"v"}}]} — observe 201 and a media stream stored with an empty tag key, where POST /tags/{FlowArn} with the same pair is refused 400 "A tag key must not be empty."
3. Repeat with "mediaStreamTags" carrying more pairs than crate::state::MAX_TAGS_PER_RESOURCE, or a key longer than MAX_TAG_KEY_LENGTH — observe 201 in each case, while the same map under "outputTags" on POST /v1/flows/{FlowArn}/outputs is refused 400.
4. Inspect the stored flow (state().flows[FlowArn].media_streams[0].tags) to confirm the unchecked pairs were persisted.
