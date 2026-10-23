Issue-Source: trace-sweep-divergence
Issue-Trace: networkmanager-site-lifecycle-with-tags
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmanager-site-lifecycle-with-tags networkmanager-link-lifecycle-and-association
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|ListTagsForResource|2xx->4xx|$.TagList
Issue-Key: networkmanager|ListTagsForResource|2xx->4xx|status
Issue-Key: networkmanager|TagResource|2xx->4xx|$
Issue-Key: networkmanager|TagResource|2xx->4xx|status
Issue-Key: networkmanager|UntagResource|2xx->4xx|$
Issue-Key: networkmanager|UntagResource|2xx->4xx|status
Issue-Key: networkmanager|GetLinks|2xx->2xx|$.Links[*]
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# networkmanager global-network scoping in ARNs breaks tag operations and GetLinks LinkArn

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
networkmanager sub-resource ARNs are mishandled with respect to their parent global network: the ARN->resource resolution used by the /tags/{ResourceArn} operations fails to find an existing site (it resolves the ARN to a composite 'global-network-.../site-...' key that matches nothing in the store), and GetLinks serializes LinkArn without the global-network segment AWS includes. This bundle carries FOUR distinct causes; all four must be closed.

Cause 1 of 4 — ListTagsForResource on an existing site's ARN returns 404 instead of its tags.
AWS: Smithy: ListTagsForResource -> GET /tags/{ResourceArn}, http code 200, output {TagList: TagList}; ResourceNotFoundException (404) is modeled only for an ARN that names no resource. The site was created with Tags in the same fixture and GetSites in that fixture returns it with Tags [{Key: purpose, Value: probe}], so the resource exists.
Emulator: 404 and no $.TagList member.
Why: tags become unreadable for any site that does exist, so clients that reconcile tags (cost allocation, tag-based discovery, IaC drift detection) see the resource as missing and either error out or try to re-create it.
Fix: the ARN parsing / resource lookup shared by the tag operations in the networkmanager crate (src/ops/ tag handlers plus the lookup in src/state.rs) must resolve a site ARN to the stored site; the key it derives must match the key the site is stored under.
Traces: networkmanager-site-lifecycle-with-tags#task#6 (representative), plus task#8 and task#10 in the same cluster.

Cause 2 of 4 — TagResource on an existing site's ARN returns 404 instead of an empty 200.
AWS: Smithy: TagResource -> POST /tags/{ResourceArn} with required Tags, output TagResourceResponse with no members, http code 200 (recording: 200 with an empty body {}).
Emulator: 404 with body {'__type': 'ResourceNotFoundException', 'Message': 'Resource not found.', 'ResourceId': 'global-network-00000000000000001/site-00000000000000002', 'ResourceType': 'site'} — the echoed ResourceId shows the handler built a global-network-scoped composite id and then failed to find it.
Why: tagging an existing site is impossible; every tag-then-verify flow fails at the first write, and clients that treat ResourceNotFoundException as terminal give up on a live resource.
Fix: same ARN->resource resolution as cause 1, in the TagResource handler.
Traces: networkmanager-site-lifecycle-with-tags#task#7.

Cause 3 of 4 — UntagResource on an existing site's ARN returns 404 instead of an empty 200.
AWS: Smithy: UntagResource -> DELETE /tags/{ResourceArn} with required TagKeys (@httpQuery tagKeys), output UntagResourceResponse with no members, http code 200 (recording: 200, empty body {}).
Emulator: 404 with the same ResourceNotFoundException body and the same composite 'global-network-.../site-...' ResourceId.
Why: tags can never be removed, so tag state diverges permanently from what the client asked for.
Fix: same ARN->resource resolution as cause 1, in the UntagResource handler.
Traces: networkmanager-site-lifecycle-with-tags#task#9.

Cause 4 of 4 — GetLinks returns a LinkArn without the global-network segment.
AWS: Smithy: GetLinks -> GET /global-networks/{GlobalNetworkId}/links, output {Links: LinkList}; Link.LinkArn targets LinkArn (@length max 500). Recorded behavior only: the link ARN is scoped by its global network, of the form arn:aws:networkmanager::<account>:link/<globalNetworkId>/<linkId> (recording example: arn:aws:networkmanager::000000000000:link/global-network-00000000000000001/link-00000000000000003).
Emulator: serializes arn:aws:networkmanager::000000000000:link/link-00000000000000003 — the <globalNetworkId>/ segment is missing. All other Link members (LinkId, SiteId, Bandwidth, Provider, Type, State) match.
Why: the ARN is the identifier clients hand to IAM policies, tagging APIs and cross-service references. An unscoped ARN will not match a resource pattern written for AWS and cannot be resolved back to the link, so authorization and tag calls built from it fail.
Fix: the Link ARN construction in the networkmanager crate (CreateLink/GetLinks serialization, src/ops/ and the stored link record in src/state.rs) must include the global-network scope.
Traces: networkmanager-link-lifecycle-and-association#task#5 (representative), plus task#7 in the same cluster.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 ListTagsForResource — expected status 200, got 404; '$.TagList: missing (expected [{'Key': 'purpose', 'Value': 'probe'}], got None)'. Cluster: networkmanager-site-lifecycle-with-tags#task#6, #task#8, #task#10.
Cause 2 TagResource — expected 200, got 404; '$: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'Message': 'Resource not found.', 'ResourceId': 'global-network-00000000000000001/site-00000000000000002', 'ResourceType': 'site'})'.
Cause 3 UntagResource — expected 200, got 404; identical ResourceNotFoundException body and composite ResourceId.
Cause 4 GetLinks — '$.Links[0]: missing (expected {... 'LinkArn': 'arn:aws:networkmanager::000000000000:link/global-network-00000000000000001/link-00000000000000003' ...}, got {... 'LinkArn': 'arn:aws:networkmanager::000000000000:link/link-00000000000000003' ...})'; status 200 on both sides and every other member equal.

The site in the tag fixture demonstrably exists on the emulator: the same fixture's GetSites (task#5) returns Sites[0] with SiteId site-00000000000000002 and Tags [{Key: purpose, Value: probe}], so the 404s are a lookup/ARN-resolution defect, not a missing resource.

Model basis: services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json — TagResource/UntagResource outputs have no members (http code 200); ListTagsForResource output {TagList}; Link.LinkArn -> LinkArn.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture networkmanager-site-lifecycle-with-tags, step task#6 (ListTagsForResource on the site ARN) -> 404 instead of 200 + TagList.
4. Cause 2: networkmanager-site-lifecycle-with-tags#task#7 (TagResource on the same ARN) -> 404 ResourceNotFoundException instead of empty 200.
5. Cause 3: networkmanager-site-lifecycle-with-tags#task#9 (UntagResource) -> 404 instead of empty 200.
6. Cause 4: networkmanager-link-lifecycle-and-association#task#5 (GetLinks) -> Links[0].LinkArn missing the /<globalNetworkId>/ segment.
