Issue-Source: trace-sweep-divergence
Issue-Trace: iotsecuretunneling-open-describe-list-close-tunnel-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsecuretunneling-open-describe-list-close-tunnel-lifecycle iotsecuretunneling-open-tunnel-with-minimal-defaults iotsecuretunneling-operate-on-closed-tunnel-fails iotsecuretunneling-rotate-tunnel-access-token-updates-destination iotsecuretunneling-timeout-and-max-results-bounds
Issue-Shape-Id: com.amazonaws.iotsecuretunneling#IoTSecuredTunneling
Issue-Key: iotsecuretunneling|DescribeTunnel|2xx->2xx|$.tunnel.tags
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# DescribeTunnel omits tunnel.tags for an untagged tunnel instead of serializing an empty list

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTunnel

## Description
Cause 1 of 1: DescribeTunnel's response drops the tunnel.tags member whenever the tunnel carries no tags, where the recorded API always serializes the member.

  AWS:      recorded behavior only. The Smithy model declares Tunnel$tags as TagList with @length(min: 1, max: 200) and says nothing about an untagged tunnel's representation, so the recordings are the authority: every recorded DescribeTunnel response for a tunnel with no tags contains tunnel.tags as an empty JSON array ([]). The rest of the tunnel projection matches; only this member diverges.
  Emulator: the member is absent from the response object entirely (replay detail "$.tunnel.tags: missing (expected [], got None)"). The handler fills it from crate::state::tag_list, which returns None both when the ARN has no tag entry and when its stored set is empty — documented there as deliberate, on the reasoning that TagList's @length min 1 "cannot carry an empty list on the wire". The recordings contradict that reasoning for this output member.
  Why:      a client cannot distinguish "no tags" from "tags not reported". Generated SDK models for a list member commonly expose it as a non-nullable collection or as a nullable one that callers iterate directly; against AWS the field is an empty list, against the emulator it is null/absent, so code that iterates tunnel.tags without a null check (Java/.NET/Go SDK consumers, and any JSONPath assertion such as $.tunnel.tags) fails only against the emulator. It also makes a round trip through TagResource/UntagResource asymmetric: after removing the last tag the member disappears rather than becoming empty.
  Fix:      DescribeTunnel's response projection for this member — the tunnel.tags value built in the operation handler from the tag store, i.e. the untagged case must yield an empty list rather than an absent member (the helper that collapses an empty tag set to None is the shared seam).
  Traces:   iotsecuretunneling-open-describe-list-close-tunnel-lifecycle#task#2 and #task#5; iotsecuretunneling-open-tunnel-with-minimal-defaults#task#2; iotsecuretunneling-operate-on-closed-tunnel-fails#task#3; iotsecuretunneling-rotate-tunnel-access-token-updates-destination#task#4; iotsecuretunneling-timeout-and-max-results-bounds#task#5 — one cluster, 6 affected steps, all with the same divergence_key service=iotsecuretunneling|DescribeTunnel|status=200->200|keys=$.tunnel.tags.

No open iotsecuretunneling issue covers this member; the open tag-related ones concern TagResource's lastUpdatedAt stamping and UntagResource's clock ordering, not the DescribeTunnel projection.

## Evidence
Replay of trace set simparity-wire-aws-v1, cluster service=iotsecuretunneling|DescribeTunnel|status=200->200|keys=$.tunnel.tags (verdict mismatch, affected_count 6).
Representative: fixture iotsecuretunneling-open-describe-list-close-tunnel-lifecycle, step task#2.
Status matches (200 expected, 200 actual); body diverges: detail "$.tunnel.tags: missing (expected [], got None)".
Expected (recorded AWS): "tunnel": { ..., "tags": [] } for a tunnel with no tags.
Actual (emulator): the "tags" key is not emitted at all.
Source: services/service-com-amazonaws-iotsecuretunneling-iotsecuredtunneling/src/ops/describe_tunnel.rs:160,187 (tags from store.tag_list(&stored.tunnel_arn)) and src/state.rs:590-603 (tag_list returns None for an empty/absent tag set).
Model: smithy_query(query="shape", shape="Tunnel") => member tags -> TagList, length min 1 max 200, no default and no guidance on the empty case.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture iotsecuretunneling-open-describe-list-close-tunnel-lifecycle.
2. Step task#1 opens a tunnel with no tags; step task#2 calls DescribeTunnel on it (likewise task#2 of iotsecuretunneling-open-tunnel-with-minimal-defaults).
3. Compare the response body against the recording: the recorded tunnel object carries "tags": [], the emulator's omits the member.
