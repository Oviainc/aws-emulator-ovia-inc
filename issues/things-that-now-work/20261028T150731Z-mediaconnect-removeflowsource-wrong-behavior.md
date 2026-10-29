Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# RemoveFlowSource removes the flow's primary source and leaves SourceFailoverConfig naming a source the flow no longer has

_Filed: 2026-10-28T15:07:30Z_
_Operation: RemoveFlowSource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RemoveFlowSource` drops the addressed source from `StoredFlow::sources` without touching the flow's `SourceFailoverConfig`. When the removed source is the one `SourcePriority.PrimarySource` names, the stored flow is left holding exactly the state this crate declares impossible: a failover setting naming a source the flow has not got. DescribeFlow then reports that dangling name, and a later UpdateFlow that re-states the failover priority is refused for the state RemoveFlowSource created.

The crate already treats reference integrity on removal as this operation family's job: its two sibling remove handlers refuse to leave a dangling reference behind (RemoveFlowMediaStream refuses while a source/output still associates the stream, RemoveFlowVpcInterface refuses while a source/output still attaches the interface). RemoveFlowSource is the one remove path that leaves one.

## Evidence

The invariant the crate states for a STORED failover setting — services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs:1361-1365:
```
/// Every operation that writes a source priority resolves the reference HERE: the create
/// paths against the sources THAT request creates, which are all the sources the new
/// resource has, and the update paths against the sources the resource already holds. So a
/// stored failover setting never names a source its resource has not got, whichever call
/// wrote it.
```
RemoveFlowSource removes a source and writes nothing else — src/ops/remove_flow_source.rs:217-221:
```
        let removed = flow.sources.remove(position);
        Ok(RemoveFlowSourceOutput {
            flow_arn: Some(flow.flow_arn.clone()),
            source_arn: Some(removed.source_arn),
        })
```
(the handler's only other check is the "more than one source" precondition at remove_flow_source.rs:206-215; `source_failover_config` is never read).

The sibling remove paths DO guard their references — src/ops/remove_flow_media_stream.rs:217-229:
```
        // "This action is only available if the media stream is not associated with a
        // source or output": an association is a fact about the STORED flow ...
        if let Some(association) = flow.media_stream_association(&input.media_stream_name) {
            return Err(bad_request(format!(
                "Media stream {:?} is associated with {association}. A media stream can be \
                 removed only while it is associated with no source and no output.",
```
and src/ops/remove_flow_vpc_interface.rs:228-234:
```
        if let Some(reference) = flow.vpc_interface_reference(&input.vpc_interface_name) {
            return Err(bad_request(format!(
                "VPC interface {:?} is referenced by {reference}. A VPC interface can be \
                 removed only while it is referenced by no source and no output.",
```
UpdateFlow refuses the resulting state — src/ops/update_flow.rs:372-381 (`flow_primary_source_rejection`, message "Source {primary:?} is not a source of this flow, so it cannot be its primary source.", state.rs:1376-1379).

Test coverage: tests/all/failover_primary_source.rs pins the rule only for the paths that WRITE a priority (its module doc, lines 15-19: "The rule binds every operation that writes a priority ... CreateFlow and CreateBridge ... while UpdateFlow and UpdateBridge ..."); no test exercises RemoveFlowSource against a failover primary, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /v1/flows with two sources named "primary" and "backup" and `{"sourceFailoverConfig":{"state":"ENABLED","failoverMode":"MERGE","sourcePriority":{"primarySource":"backup"}}}` — 201.
2. DELETE /v1/flows/{FlowArn}/source/{SourceArn} for the source named "backup" — observe 202 and no refusal.
3. GET /v1/flows/{FlowArn} — observe `flow.sourceFailoverConfig.sourcePriority.primarySource == "backup"` while `flow.sources` holds only "primary".
4. PUT /v1/flows/{FlowArn} with `{"sourceFailoverConfig":{"sourcePriority":{"primarySource":"backup"}}}` — observe 400 BadRequestException "Source \"backup\" is not a source of this flow, so it cannot be its primary source." for the exact state step 2 stored.
