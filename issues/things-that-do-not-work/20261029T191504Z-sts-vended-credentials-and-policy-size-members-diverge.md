Issue-Source: trace-sweep-divergence
Issue-Trace: sts-access-key-info-for-known-keys
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-access-key-info-for-known-keys sts-assume-role-basic-success sts-assume-role-session-name-length-bounds sts-federation-token-name-length-bounds sts-get-federation-token-basic-success
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|GetSessionToken|2xx->2xx|$.GetSessionTokenResponse.GetSessionTokenResult.Credentials.AccessKeyId
Issue-Key: sts|AssumeRole|2xx->2xx|$.AssumeRoleResponse.AssumeRoleResult.Credentials.AccessKeyId
Issue-Key: sts|AssumeRole|2xx->2xx|$.AssumeRoleResponse.AssumeRoleResult.Credentials.SessionToken
Issue-Key: sts|AssumeRole|2xx->2xx|$.AssumeRoleResponse.AssumeRoleResult.PackedPolicySize
Issue-Key: sts|AssumeRole|2xx->2xx|$.AssumeRoleResponse.AssumeRoleResult.SessionTokenUtilization
Issue-Key: sts|GetFederationToken|2xx->2xx|$.GetFederationTokenResponse.GetFederationTokenResult.Credentials.AccessKeyId
Issue-Key: sts|GetFederationToken|2xx->2xx|$.GetFederationTokenResponse.GetFederationTokenResult.PackedPolicySize
Issue-Key: sts|GetFederationToken|2xx->2xx|$.GetFederationTokenResponse.GetFederationTokenResult.SessionTokenUtilization
Issue-Key: sts|GetFederationToken|2xx->2xx|$.GetFederationTokenResponse.GetFederationTokenResult.Credentials.SessionToken
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# STS vended credentials and policy-size members diverge: non-AWS access key ids, PackedPolicySize always 0

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
Every successful STS credential-vending response in the corpus diverges in its result members, in two related ways produced by the same credential-minting component: the Credentials the emulator mints are not shaped like AWS temporary credentials, and the packed-policy size members are not computed from the request at all (PackedPolicySize is hard 0, SessionTokenUtilization is a constant). The statuses are correct (200) in all six causes — only the body members diverge. 6 causes, all on service sts.

Cause 1 of 6: GetSessionToken Credentials.AccessKeyId is not shaped like an AWS temporary-credential access key id.
AWS: Smithy: Credentials.AccessKeyId is required, target accessKeyIdType, @length 16-128, @pattern ^[\w]*$. AWS temporary credentials (GetSessionToken, AssumeRole*, GetFederationToken) carry a 20-character uppercase-alphanumeric key id whose fixed prefix is 'ASIA' (long-term IAM user keys use 'AKIA'); the remainder is service-generated and differs per call. The fixture asserts the SHAPE of the value, not the recorded value.
Emulator: the key id is an 'EMULATOR'-prefixed literal with a short random tail (20 characters, e.g. the 'EMULATORAS...' form seen in the replay), which fails the temporary-credential shape.
Why: anything that classifies a credential by its key-id prefix — the emulator's own GetAccessKeyInfo/authorization lookups, client-side assertions that a session is temporary, log scrubbers and credential-type heuristics — misclassifies these credentials; a test that checks 'this is a temporary (ASIA) session' cannot pass against the emulator.
Fix: the sts crate's credential-minting helper (shared by all vending operations) — generate AccessKeyId as a 20-character uppercase-alphanumeric id with the ASIA prefix for temporary sessions, instead of embedding an 'EMULATOR' marker.
Traces: sts-access-key-info-for-known-keys#task#1 (also task#3).

Cause 2 of 6: AssumeRole Credentials.AccessKeyId and Credentials.SessionToken are both off-shape.
AWS: Smithy: Credentials.AccessKeyId as in Cause 1; Credentials.SessionToken is required, target tokenType, and is documented as 'the token that users must pass to the service API to use the temporary credentials'. The recorded token is an opaque base64 blob that is itself a versioned, service-decodable envelope (its leading marker is fixed by STS and identical across calls); the fixture asserts that shape, not the value.
Emulator: the AccessKeyId is the 'EMULATOR'-prefixed form of Cause 1 and the SessionToken is an unstructured random base64 string with no STS envelope marker, so neither satisfies the recorded shape.
Why: a client that round-trips the session token (passing it back to the emulator or carrying it into a signed request) cannot rely on the token being recognizable as an STS session token, and any assertion on temporary-credential shape fails; the credentials cannot stand in for AWS ones in a fidelity test.
Fix: the credential-minting helper as in Cause 1, extended to SessionToken — mint a token whose encoding carries the STS session envelope marker rather than raw random bytes.
Traces: sts-assume-role-basic-success#task#2.

Cause 3 of 6: AssumeRole reports PackedPolicySize 0 and a constant SessionTokenUtilization for a request that does carry a session policy.
AWS: Smithy: AssumeRoleResponse.PackedPolicySize (nonNegativeIntegerType, @range min 0, deprecated since 2026-06-17 in favour of SessionTokenUtilization) is documented as 'A percentage value that indicates the packed size of the session policies and session tags combined passed in the request', and SessionTokenUtilization (@range min 0) is its replacement carrying the same percentage. Both are therefore derived from the request's Policy/PolicyArns/Tags: a request with a session policy yields a non-zero percentage, and the two members report the SAME percentage for the same request.
Emulator: PackedPolicySize is 0 and SessionTokenUtilization is a fixed value independent of the request (the replay shows both members disagreeing with the recording, which reported one and the same non-zero percentage in both).
Why: a caller watching PackedPolicySize/SessionTokenUtilization to stay clear of the 100-percent packed-policy ceiling (past which AWS fails the request with PackedPolicyTooLarge) sees 0 regardless of how large its session policy and tags are, so the headroom signal is useless and the PackedPolicyTooLarge path can never be approached or tested.
Fix: the AssumeRole handler's response construction plus the shared packed-policy accounting — compute the packed size of the session policies and tags actually supplied, express it as a percentage of the allowance, and report that same value in both PackedPolicySize and SessionTokenUtilization (0 only when nothing was passed).
Traces: sts-assume-role-session-name-length-bounds#task#3.

Cause 4 of 6: GetFederationToken reports PackedPolicySize 0 for a request carrying a policy (and the Cause 1 key-id shape).
AWS: Smithy: GetFederationTokenResponse.PackedPolicySize, same documented percentage semantics as Cause 3; GetFederationToken takes Policy/PolicyArns, so a supplied policy must produce a non-zero percentage.
Emulator: PackedPolicySize is 0 while the recording reported a non-zero percentage, and Credentials.AccessKeyId carries the 'EMULATOR' prefix.
Why: as Cause 3, on the federation path.
Fix: the GetFederationToken handler's response construction, over the shared packed-policy accounting; plus the credential-minting helper for the key id.
Traces: sts-federation-token-name-length-bounds#task#1.

Cause 5 of 6: GetFederationToken's PackedPolicySize and SessionTokenUtilization disagree with each other and with the request.
AWS: as Cause 3/4 — the two members report the same request-derived percentage.
Emulator: PackedPolicySize is 0 while SessionTokenUtilization is a constant unrelated to the policy supplied, so the pair is internally inconsistent as well as wrong; the AccessKeyId is again the 'EMULATOR' form.
Why: a client reading either member (the deprecated one or its replacement) gets a different answer, so neither can be trusted and migration from PackedPolicySize to SessionTokenUtilization cannot be validated against the emulator.
Fix: as Cause 4 — both members must be filled from one computation.
Traces: sts-federation-token-name-length-bounds#task#3.

Cause 6 of 6: GetFederationToken Credentials.AccessKeyId and Credentials.SessionToken are both off-shape on the basic success path.
AWS: Credentials.AccessKeyId / Credentials.SessionToken as in Causes 1-2; GetFederationToken vends temporary credentials, so the ASIA-prefixed key id and the STS session-token envelope apply here too.
Emulator: 'EMULATOR'-prefixed key id and an unstructured random base64 session token.
Why: as Cause 2, for federated sessions.
Fix: the credential-minting helper shared by GetFederationToken.
Traces: sts-get-federation-token-basic-success#task#1.

Related but NOT covering this: the open issue 'cognito-identity: GetCredentialsForIdentity under a Token role mapping vends an access key id prefixed EMULATOR instead of an STS temporary-credential...' (20261029T071507Z-cognito-identity-...) reports the same 'EMULATOR' prefix in the cognito-identity crate; it carries no sts replay key, so the sts vending path is still unfixed.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false). All six clusters are status 200 -> 200; only body members diverged.

Cause 1 — sts-access-key-info-for-known-keys#task#1, GetSessionToken: '$.GetSessionTokenResponse.GetSessionTokenResult.Credentials.AccessKeyId: value_mismatch (expected <shaped-not-asserted>, got EMULATORASSOFJPFPBEU)'; cluster also covers task#3.
Cause 2 — sts-assume-role-basic-success#task#2, AssumeRole: 'Credentials.AccessKeyId: value_mismatch (expected <shaped-not-asserted>, got EMULATORASCVB4NYVZ4O)' and 'Credentials.SessionToken: value_mismatch (expected <shaped-not-asserted>, got <1.5KB of unstructured base64 with no STS envelope marker>)'.
Cause 3 — sts-assume-role-session-name-length-bounds#task#3, AssumeRole: 'PackedPolicySize: value_mismatch (expected 19, got 0)', 'SessionTokenUtilization: value_mismatch (expected 19, got 17)', 'Credentials.AccessKeyId: value_mismatch (expected <shaped-not-asserted>, got EMULATORAS4PXA6JCLWD)'. The recording reports the SAME percentage in both members; the emulator reports 0 and a constant.
Cause 4 — sts-federation-token-name-length-bounds#task#1, GetFederationToken: 'PackedPolicySize: value_mismatch (expected 17, got 0)', 'Credentials.AccessKeyId: value_mismatch (expected <shaped-not-asserted>, got EMULATORAS65XOTYICVH)'.
Cause 5 — sts-federation-token-name-length-bounds#task#3, GetFederationToken: 'PackedPolicySize: value_mismatch (expected 16, got 0)', 'SessionTokenUtilization: value_mismatch (expected 16, got 17)' — the emulator's SessionTokenUtilization is the same 17 it returned for a different request in Cause 3, confirming it is a constant — plus 'Credentials.AccessKeyId: ... got EMULATORAS7DBRZCN4TI'.
Cause 6 — sts-get-federation-token-basic-success#task#1, GetFederationToken: 'Credentials.AccessKeyId: value_mismatch (expected <shaped-not-asserted>, got EMULATORASRXZZ3CT2GN)' and 'Credentials.SessionToken: value_mismatch (expected <shaped-not-asserted>, got <unstructured base64>)'.

The specific recorded percentages (19/17/16) and the specific recorded key ids/tokens are per-request values, not requirements: the requirements are that the key id carries the ASIA temporary-credential shape, that the session token is an STS session envelope, and that both size members report one percentage computed from the request's policies and tags.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and read the sts divergence clusters.
3. Cause 1: replay sts-access-key-info-for-known-keys steps task#1/task#3 — call GetSessionToken and inspect Credentials.AccessKeyId: it is 'EMULATOR'-prefixed, not an ASIA-prefixed 20-character temporary key id.
4. Cause 2: replay sts-assume-role-basic-success step task#2 — call AssumeRole and inspect Credentials.AccessKeyId and Credentials.SessionToken.
5. Cause 3: replay sts-assume-role-session-name-length-bounds step task#3 — call AssumeRole WITH a session policy and compare PackedPolicySize (0) and SessionTokenUtilization (constant) against the request's packed policy/tag size; they must be equal and non-zero.
6. Cause 4: replay sts-federation-token-name-length-bounds step task#1 — GetFederationToken with a policy; PackedPolicySize is 0.
7. Cause 5: replay sts-federation-token-name-length-bounds step task#3 — GetFederationToken with a differently sized policy; PackedPolicySize is 0 and SessionTokenUtilization is the same constant as step 5, proving it is not derived from the request.
8. Cause 6: replay sts-get-federation-token-basic-success step task#1 — inspect Credentials.AccessKeyId and Credentials.SessionToken.
9. All six steps must pass before this issue is closed.
