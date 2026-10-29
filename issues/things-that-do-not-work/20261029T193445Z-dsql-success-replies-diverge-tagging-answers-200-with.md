Issue-Source: trace-sweep-divergence
Issue-Trace: dsql-cluster-tagging-workflow
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dsql-cluster-tagging-workflow dsql-cluster-deletion-protection-conflict dsql-cluster-update-config dsql-cluster-policy-version-guards dsql-cluster-policy-workflow dsql-vpc-endpoint-service-name
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql|TagResource|2xx->2xx|status
Issue-Key: dsql|UntagResource|2xx->2xx|status
Issue-Key: dsql|GetCluster|2xx->2xx|$.tags
Issue-Key: dsql|GetClusterPolicy|2xx->2xx|$.policy
Issue-Key: dsql|PutClusterPolicy|2xx->2xx|$.policyVersion
Issue-Key: dsql|GetVpcEndpointServiceName|2xx->2xx|$.serviceName
Issue-Commit: 16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22

# dsql success replies diverge: tagging answers 200 with a body, and read paths drop or re-render modeled values

> _Note: this issue was identified against the code base at commit `16092bb1fe4b1c4e1f6f331d6e3c413bf3cfad22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Six successful replies differ from Aurora DSQL's: the two tagging mutations use the wrong HTTP success status, and four read/write paths omit a modeled member or render it in a form the service does not use.

Cause 1 of 6: TagResource's success status.
AWS: 204 No Content. Smithy: TagResource's output is Unit — there is no output structure and the model's 200 is a DEFAULTED code (http {code: 200, codeDefaulted: true}), so the modelled 200 is not what the service sends; the recording shows 204 with no body.
Emulator: 200.
Why: a client that branches on the exact success status (or treats 200-with-no-body as a protocol error) mis-handles a successful tagging call, and status-asserting contract tests diverge.
Fix: TagResource's serializer — emit 204 for the Unit output.
Traces: dsql-cluster-tagging-workflow#task#4.

Cause 2 of 6: UntagResource's success status.
AWS: 204 No Content (output Unit, code likewise defaulted in the model; recorded behavior is 204).
Emulator: 200.
Why: as Cause 1, on the untag path.
Fix: UntagResource's serializer.
Traces: dsql-cluster-tagging-workflow#task#6.

Cause 3 of 6: GetCluster omits the tags member for an untagged cluster.
AWS: serializes tags as an empty map {} rather than omitting it. Smithy: GetClusterOutput.tags targets TagMap (@length 0..200) — an empty map is a legal, and the recorded, value.
Emulator: the tags key is absent from the body.
Why: a client that reads response.tags without a null guard breaks, and code that distinguishes "no tags" from "tags not reported" cannot tell the two apart.
Fix: GetCluster's serializer/response builder — always emit tags, empty map included.
Traces: dsql-cluster-deletion-protection-conflict#task#5 (also affects dsql-cluster-update-config#task#4 and #task#6).

Cause 4 of 6: GetClusterPolicy hands back the policy document re-serialized in compact form.
AWS: returns the attached document in the service's canonical pretty-printed rendering — two-space indentation and " : " between key and value — rather than the compact form (recorded behavior only; the model types PolicyDocument as a plain string, so the model states no formatting rule). The requirement is that the document the service hands back is rendered in that canonical form for the same stored policy, whatever its statements are.
Emulator: returns a minified single-line JSON rendering of the same document.
Why: a client that diffs or hashes the returned policy against what the service last returned sees a spurious change on every read, and byte-exact policy comparison never matches.
Fix: GetClusterPolicy's serializer (the policy-document rendering).
Traces: dsql-cluster-policy-version-guards#task#7 (also affects dsql-cluster-policy-workflow#task#5 and #task#7).

Cause 5 of 6: PutClusterPolicy's policyVersion is a small ordinal counter.
AWS: returns an opaque version token in the shape the service mints for PolicyVersion (Smithy: PutClusterPolicyOutput.policyVersion is a required PolicyVersion string, opaque to the client); the recorded value does not have the form of a decimal sequence number.
Emulator: returns "1" — a per-cluster counter rendered as a string.
Why: a client that round-trips the value into expectedPolicyVersion is fine, but one that validates or stores it as the service's token form diverges, and the counter makes versions guessable/forgeable across clusters in a way the real token is not.
Fix: PutClusterPolicy's policy-version minting in the cluster-policy state.
Traces: dsql-cluster-policy-workflow#task#4 (also affects dsql-cluster-policy-workflow#task#6).

Cause 6 of 6: GetVpcEndpointServiceName embeds the full 26-character cluster identifier in the service name.
AWS: returns com.amazonaws.<region>.dsql-<short per-cluster suffix> — a short token (4 characters in the recording), not the cluster identifier. Smithy: ServiceName @length 1..128, pattern ^com\\.amazonaws\\.[a-z0-9-]+\\.dsql[a-z0-9-]*$. The generated suffix value itself is per-environment and is NOT the requirement; its FORM is — a short suffix distinct from the cluster id.
Emulator: returns com.amazonaws.<region>.dsql-<full 26-char cluster identifier>, i.e. it derives the endpoint service name from the identifier.
Why: a client that parses the service name, or that asserts the cluster identifier is not exposed through the VPC endpoint service name, sees a different structure than AWS produces; length-sensitive consumers get a far longer name.
Fix: GetVpcEndpointServiceName's handler — mint and store a short per-cluster endpoint-service suffix instead of reusing the cluster identifier.
Traces: dsql-vpc-endpoint-service-name#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1:
- dsql-cluster-tagging-workflow#task#4 TagResource: expected status 204, got 200.
- dsql-cluster-tagging-workflow#task#6 UntagResource: expected status 204, got 200.
- dsql-cluster-deletion-protection-conflict#task#5 GetCluster (200 both sides): $.tags expected {}, got None (cluster affects dsql-cluster-update-config#task#4, #task#6).
- dsql-cluster-policy-version-guards#task#7 GetClusterPolicy (200 both sides): $.policy expected the pretty-printed document ('{\n  "Version" : "2012-10-17",\n  "Statement" : [ {...} ]\n}'), got the same document minified ('{"Version":"2012-10-17","Statement":[{...}]}') (cluster affects dsql-cluster-policy-workflow#task#5, #task#7).
- dsql-cluster-policy-workflow#task#4 PutClusterPolicy (200 both sides): $.policyVersion did not match the recorded value's shape; got '1' (cluster affects dsql-cluster-policy-workflow#task#6).
- dsql-vpc-endpoint-service-name#task#3 GetVpcEndpointServiceName (200 both sides): $.serviceName expected the form 'com.amazonaws.us-west-2.dsql-<short suffix>' (recorded 'com.amazonaws.us-west-2.dsql-7cwu'), got 'com.amazonaws.us-west-2.dsql-<full cluster id>'.

## Steps to Reproduce
1. Replay dsql-cluster-tagging-workflow steps task#4 (TagResource) and task#6 (UntagResource) -> emulator answers 200 where the recording shows 204.
2. Replay dsql-cluster-deletion-protection-conflict step task#5 (GetCluster on an untagged cluster) -> response body has no `tags` key; AWS returns tags: {}.
3. Replay dsql-cluster-policy-version-guards step task#7 (GetClusterPolicy after a policy put) -> $.policy is minified instead of the service's pretty-printed rendering.
4. Replay dsql-cluster-policy-workflow step task#4 (PutClusterPolicy) -> $.policyVersion is '1'.
5. Replay dsql-vpc-endpoint-service-name step task#3 (GetVpcEndpointServiceName) -> $.serviceName embeds the 26-character cluster identifier instead of a short per-cluster suffix.
