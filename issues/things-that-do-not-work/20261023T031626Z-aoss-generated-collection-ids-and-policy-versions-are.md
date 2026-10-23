Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-access-policy-json-document-byte-preservation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-access-policy-json-document-byte-preservation opensearchserverless-collection-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|CreateCollection|2xx->2xx|$.createCollectionDetail.id
Issue-Related: 20261023T031626Z-aoss-writes-are-not-reflected-in-stored-state.md
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss generated collection ids and policy versions are fixed placeholder values

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPolicy

## Description
This issue bundles TWO distinct causes in the opensearchserverless (`aoss`) emulator: the service's two server-minted opaque identifiers are emitted as fixed, structurally implausible placeholders that fail the fixtures' shape assertions. Each cause must be closed before submitting.

Cause 1 of 2: policyVersion is minted as a monotonic hex counter rather than an opaque version token of AWS's form.
AWS: Smithy PolicyVersion is a string with length 20..36 and pattern ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$ — i.e. a base64-shaped token; recorded responses carry a value of that form, and the fixture asserts its SHAPE (not an exact value, since the value differs per environment). Each create/update mints a distinct version used for optimistic concurrency on Update*Policy.
Emulator: emits values like '000001a166bcb82700000001' — a 24-char zero-padded hex counter with an embedded timestamp. Hypothesis: a counter/timestamp formatter is used where a base64-shaped opaque token is expected; the fixture's shape assertion for this member rejects it.
Why: a client that round-trips policyVersion works, but anything that validates or parses the token against the documented base64 shape (and any recording-based test that asserts the identifier's form) diverges; the embedded wall-clock value also makes the token non-opaque.
Fix: the policy-version minting in this service's state (services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs and the Create*/Update* policy handlers that call it).
Traces: opensearchserverless-access-policy-json-document-byte-preservation#task#2.

Cause 2 of 2: CreateCollection mints a constant collection id.
AWS: Smithy CollectionId is a string with length 3..40 and pattern ^[a-z0-9]{3,40}$; recorded behavior is a distinct, randomly-shaped 24-character id per collection, and the fixture asserts its shape rather than a literal.
Emulator: returns '8sh3tqaaaaaaaaaaaaaaaaaa' — a short prefix followed by filler 'a's — and the SAME literal appears as the collection-group id in other fixtures of this run (e.g. the collection-group lifecycle teardown detail carries id '8sh3tqaaaaaaaaaaaaaaaaaa'), so the id is a fixed placeholder shared across resources rather than a per-resource identifier.
Why: ids are the primary key for DeleteCollection/BatchGetCollection/BatchGetCollectionGroup and appear inside ARNs; a constant id means two resources collide in any client-side map or ARN, and no test can distinguish them. Error messages that embed the id (see the error-status issue for this service) then leak the same placeholder.
Fix: the collection-id (and collection-group-id) generator in this service's state, so each resource gets a distinct id matching ^[a-z0-9]{3,40}$.
Traces: opensearchserverless-collection-lifecycle#setup#2.

## Evidence
Cause 1 — opensearchserverless-access-policy-json-document-byte-preservation#task#2 GetAccessPolicy (200): $.accessPolicyDetail.policyVersion expected '<shaped-not-asserted>' (a shape assertion on the identifier), got '000001a166bcb82700000001'. The same hex-counter form appears in other fixtures' policyVersion values ('000001a166bcba9200000001', '000001a166bdfa4900000001').
Cause 2 — opensearchserverless-collection-lifecycle#setup#2 CreateCollection (200): $.createCollectionDetail.id expected '<shaped-not-asserted>', got '8sh3tqaaaaaaaaaaaaaaaaaa'. The identical literal is served as a collection-GROUP id in opensearchserverless-collection-group-lifecycle#teardown#6 ('arn:aws:aoss:us-west-2:000000000000:collection-group/8sh3tqaaaaaaaaaaaaaaaaaa'), showing the value is a fixed placeholder, not a minted per-resource id.

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures.
1 (Cause 1): CreateAccessPolicy(type='data', name=N, policy=P) then GetAccessPolicy(type='data', name=N) -> expect accessPolicyDetail.policyVersion to be an opaque token of the modeled base64 shape (length 20..36, ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$); observe a zero-padded hex counter such as '000001a166bcb82700000001'. Fixture opensearchserverless-access-policy-json-document-byte-preservation#task#2.
2 (Cause 2): CreateCollection(name=N1) and CreateCollection(name=N2), plus CreateCollectionGroup(name=N3) -> expect three DISTINCT ids matching ^[a-z0-9]{3,40}$; observe the same literal '8sh3tqaaaaaaaaaaaaaaaaaa' reused. Fixture opensearchserverless-collection-lifecycle#setup#2.
