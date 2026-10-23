Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-domain-permissions-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-domain-permissions-policy-lifecycle codeartifact-domain-permissions-policy-workflow codeartifact-repository-permissions-policy codeartifact-repository-permissions-policy-lifecycle
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|PutDomainPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|PutDomainPermissionsPolicy|2xx->2xx|$.policy.revision
Issue-Key: codeartifact|GetDomainPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|DeleteDomainPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|PutRepositoryPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|GetRepositoryPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|DeleteRepositoryPermissionsPolicy|2xx->2xx|$.policy.document
Issue-Key: codeartifact|PutRepositoryPermissionsPolicy|2xx->2xx|$.policy.revision
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact resource policies are re-serialized instead of stored and echoed verbatim

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDomainPermissionsPolicy

## Description
Two causes, both in how the resource-policy document is persisted and serialized. Both must be closed before submitting.

Cause 1 of 2: the policy document is round-tripped through a JSON parser and re-printed, so the returned `document` is not the string the caller supplied.
AWS: PolicyDocument is an opaque string member (Smithy: PutDomainPermissionsPolicy.policyDocument -> PolicyDocument, required, @length 1..7168, @pattern ^[\P{C}\s]+$; ResourcePolicy.document -> PolicyDocument, documented as 'The resource policy formatted in JSON'). The recorded responses return the document byte-for-byte as submitted — compact separators, no space after ':' or ',' — from Put, from Get and from Delete, for both the domain and the repository policy.
Emulator: returns a re-serialized document with Python-style separators ('{"Version": "2012-10-17", "Statement": [...]}'). The stored value is the pretty-printed form, so every later read of the same policy also diverges.
Why: the document is an opaque blob to clients. Code that diffs the policy it just wrote against what the service returns (idempotence checks, drift detection, config-as-code reconcilers), or that hashes/signs the document, sees a change on every write even when nothing changed — producing spurious 'policy drifted' results and unbounded update loops.
Fix: the domain/repository permissions-policy handlers and their state — store the submitted PolicyDocument string as received and echo it unmodified; validation may parse it, but must not replace it with its own rendering.
Traces: codeartifact-domain-permissions-policy-lifecycle#task#3 (representative, PutDomainPermissionsPolicy), codeartifact-domain-permissions-policy-lifecycle#task#4 (GetDomainPermissionsPolicy), codeartifact-domain-permissions-policy-lifecycle#task#6 (DeleteDomainPermissionsPolicy), codeartifact-domain-permissions-policy-workflow#task#3 (Put), codeartifact-repository-permissions-policy#task#5 (Get), codeartifact-repository-permissions-policy#task#6 (Delete), codeartifact-repository-permissions-policy-lifecycle#task#4 (Put).

Cause 2 of 2: a scalar `Action` in the submitted policy is rewritten into a single-element array.
AWS: recorded behavior only for the document's internal structure — the document is opaque, so a statement submitted with `"Action":"codeartifact:ReadFromRepository"` comes back with that same scalar.
Emulator: PutRepositoryPermissionsPolicy returns `"Action": ["codeartifact:ReadFromRepository"]`, i.e. the handler is normalizing IAM statement members while re-serializing. This is a structural rewrite of caller data, not only whitespace.
Why: clients that read `Statement[].Action` as the type they wrote it with (string vs list) break on the type change, and any exact-match comparison of the stored policy fails permanently.
Fix: same component as cause 1 — the repository permissions-policy handler must not normalize statement members; removing the parse/re-emit step fixes both causes.
Traces: codeartifact-repository-permissions-policy#task#4.

Not asserted here: the same clusters also show `policy.revision` differing (recorded 'EyPrTkTl3ui3QHCl4CDJXECOsAsV+85VZoMWAVPgkDk=' vs emulator '53dbe503277ef8c8f802f7010f47ca7c'). PolicyRevision is an opaque generated token (Smithy: @length 1..100, @pattern ^\S+$) and both values satisfy the model, so the revision VALUE is not a requirement and is not part of this issue — only the document must match.

## Evidence
Cause 1 — codeartifact-domain-permissions-policy-lifecycle#task#3, PutDomainPermissionsPolicy, 200/200:
  $.policy.document expected: {"Version":"2012-10-17","Statement":[{"Sid":"sim2realDomainRead","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["codeartifact:DescribeDomain","codeartifact:ListRepositoriesInDomain"],"Resource":"*"}]}
  $.policy.document actual:   {"Version": "2012-10-17", "Statement": [{"Sid": "sim2realDomainRead", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::000000000000:root"}, "Action": ["codeartifact:DescribeDomain", "codeartifact:ListRepositoriesInDomain"], "Resource": "*"}]}
  The same expected/actual pair recurs on GetDomainPermissionsPolicy (#task#4), DeleteDomainPermissionsPolicy (#task#6), PutDomainPermissionsPolicy in codeartifact-domain-permissions-policy-workflow#task#3, and on the repository policy in codeartifact-repository-permissions-policy#task#5 / #task#6 and codeartifact-repository-permissions-policy-lifecycle#task#4.

Cause 2 — codeartifact-repository-permissions-policy#task#4, PutRepositoryPermissionsPolicy, 200/200:
  $.policy.document expected: ..."Action":"codeartifact:ReadFromRepository","Resource":"*"...
  $.policy.document actual:   ..."Action": ["codeartifact:ReadFromRepository"], "Resource": "*"...
  (scalar Action turned into a one-element list, in addition to the whitespace rewrite)

## Steps to Reproduce
1. Replay codeartifact-domain-permissions-policy-lifecycle step task#3: PUT /v1/domain/permissions/policy with a compact JSON policyDocument (no spaces after ':' or ','), then compare $.policy.document to the submitted string -> whitespace reinserted.
2. Replay steps task#4 and task#6 of the same fixture: Get and Delete return the same re-serialized document, confirming the stored value was replaced.
3. Replay codeartifact-repository-permissions-policy step task#4: PUT /v1/repository/permissions/policy with a statement whose Action is a bare string -> the response contains Action as a single-element array.
