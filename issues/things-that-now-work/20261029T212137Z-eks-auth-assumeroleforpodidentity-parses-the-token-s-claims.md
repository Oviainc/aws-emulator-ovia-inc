Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks-auth.pod-identity-token-claims-are-never-evaluated
Issue-Shape-Id: com.amazonaws.eksauth#EKSAuthFrontend
Issue-Key: eks-auth.pod-identity-token-claims-are-never-evaluated|run#15|AssertionError|assert len(distinct) == 1, (
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# eks-auth AssumeRoleForPodIdentity parses the token's CLAIMS segment, so a non-JSON payload is refused as a parse failure instead of the kid-resolution rejection AWS returns (1 cause)

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleForPodIdentity

## Description
REOPENS 20261029T173232Z-eks-auth-assumeroleforpodidentity-assigns-token-header-rejections-to.md: its fix left the call refused. That fixed issue re-layered `parse_service_account_token` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs so the header layers (absent `alg` -> parse failure, non-RS256 `alg`, blank/absent `kid`, unresolvable `kid`) report AWS's identities, and the header-layer check now passes. The call is still refused with the wrong identity on a neighbouring layer: the same function ALSO insists the token's second (claims) segment decode to a JSON object, a layer real AWS never reaches, so the rejection identity depends on the claim body. The still-open issue 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md lists this same check id, but carries it under the replay key for its `assert code == "InvalidTokenException"` assertion (Cause 1, the ResourceNotFoundException-before-token-parse ordering), which now passes; the message-level divergence below is described in no open issue.

Cause 1 of 1: the claims segment is parsed and judged, so a token whose payload is not a JSON object is refused with a different InvalidTokenException message than one whose payload is a well-formed claim set, although both carry the same unresolvable `kid`.

AWS: Smithy (services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json): AssumeRoleForPodIdentity (POST /clusters/{clusterName}/assume-role-for-pod-identity, restJson1) models InvalidTokenException @httpError 400 (doc "The specified Kubernetes service account token is invalid.") and also ExpiredTokenException @httpError 400; input `token` targets JwtToken, @required, @sensitive, @length min 1, @pattern ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$. The model says nothing about which message belongs to which defect, so the layer boundary is recorded behavior only: measured against real eks-auth in us-west-2, validation of a presented token stops at resolving the JOSE header's `kid` against the named cluster's OIDC JWKS, and NOTHING past the header is ever read. Every token sharing one well-formed RS256 header with an unresolvable `kid` therefore gets exactly one identity, HTTP 400 ('InvalidTokenException', 'The token included in the request has invalid header - kid.'), whatever its payload is — inside its validity window, expired an hour ago, addressed to the wrong audience, an empty claim object, or a payload that is not even JSON. The message is entirely fixed text apart from the header member name (`kid`); no part of the @sensitive token appears in it. A consequence AWS's recording states directly: the modeled ExpiredTokenException is unreachable on this operation, because `exp` is never read.

Emulator: the five payload variants produce TWO distinct identities instead of one. Four of them answer the correct HTTP 400 ('InvalidTokenException', 'The token included in the request has invalid header - kid.'), but a payload that does not base64url-decode to a JSON object answers HTTP 400 ('InvalidTokenException', 'Failed to parse the token included in the request.'). Code grounding: services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs `parse_service_account_token` runs `let _ = json_segment(claims, JwtSegment::Claims)?;` on the claims segment BEFORE any header layer, yielding TokenMalformation::NotJson(Claims), which `malformed_token` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs words as the parse-failure text. So the emulator adds a payload layer AHEAD of the header layers AWS stops at, and that layer is observable to the caller.

Why: the EKS Pod Identity Agent and the SDK credential provider surface this message as the reason a pod could not obtain credentials, and operators and tests match on it to tell "the cluster's signing key rotated / this kid is unknown" from "what I sent is not a token at all". Because the emulator discriminates on the payload, two tokens AWS makes indistinguishable become distinguishable, and the diagnosis points at the payload when the reason AWS gives is the key id. It also means the emulator reads claims at all on this path, which is the precondition for implementing the modeled-but-unreachable ExpiredTokenException/AccessDeniedException taxonomy that real AWS never produces here. HTTP status and error code are correct; only the message (and the layer ordering it exposes) diverges.

Fix: this operation's token parsing — `parse_service_account_token` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs must not judge the claims segment at all on this path: once the token has three non-empty base64url segments, validation must proceed straight to the JOSE header layers, so the payload's content (JSON or not, expired or not, any audience) cannot change the rejection identity. The parse-failure wording in `malformed_token` stays as it is for the header/structural variants; it must simply stop being reachable via a claims-segment verdict, and no claim may gate a reply with ExpiredTokenException or AccessDeniedException.

Traces: eks-auth.pod-identity-token-claims-are-never-evaluated.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 9/22 passed; eks-auth 8/21 (13 failed), eks__eks-auth 1/1 (passed). Of the eks-auth failures, 11 are ALREADY FILED (blocked by unimplemented EC2 CreateInternetGateway) and 1 is OWNED by 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md (eks-auth.assume-role-access-denied-explicit-deny). The single remaining failure is the one filed here.

[mismatch] eks-auth.pod-identity-token-claims-are-never-evaluated, at check_eks_auth_pod_identity_token_claims_are_never_evaluated.py line 122:
  assert len(distinct) == 1
  AssertionError: the claims ARE being evaluated: five semantically different claim bodies produced 2 distinct (code, message) identities, [('InvalidTokenException', 'Failed to parse the token included in the request.'), ('InvalidTokenException', 'The token included in the request has invalid header - kid.')]; real AWS collapses all five onto ('InvalidTokenException', 'The token included in the request has invalid header - kid.')

Requests that produced it (clusterName held CONSTANT at a pattern-valid, unregistered name; header held CONSTANT at {"alg":"RS256","typ":"JWT","kid":"<44-char unresolvable key id>"}; signature segment constant; the ONLY thing that differs is the payload segment):
  - payload = base64url({"aud":["pods.eks.amazonaws.com"],"iss":"https://oidc.eks.<region>.amazonaws.com/id/<absent-cluster-id>","sub":"system:serviceaccount:default:<sa>","iat":<now>,"exp":<now+3600>}) -> emulator and AWS agree: 400 InvalidTokenException 'The token included in the request has invalid header - kid.'
  - same claim set with iat=<now-7200>, exp=<now-3600> (expired) -> agrees
  - same claim set with aud=["sts.amazonaws.com"] (wrong audience) -> agrees
  - payload = base64url({}) -> agrees
  - payload = base64url(b"<non-JSON bytes>") -> DIVERGES: emulator answers 400 InvalidTokenException 'Failed to parse the token included in the request.'; AWS answers 400 InvalidTokenException 'The token included in the request has invalid header - kid.'
  Message anatomy: in 'The token included in the request has invalid header - kid.' the only variable part is the header member name (`kid`); the rest is fixed text. 'Failed to parse the token included in the request.' is wholly fixed text and belongs only to a value that is not a readable JWT at all. No part of the @sensitive token value appears in either.

Discriminating control in the same check (asserted separately, and the emulator satisfies it): dropping `kid` from the header DOES move the identity to ('InvalidTokenException', 'The token included in the request has missing header - kid.'), so the collapse asserted above is a measurement of the claims layer, not a tautology. The check's assertion (a) — every variant is code InvalidTokenException — also passes now, so the previously-filed ResourceNotFoundException-first ordering is no longer what fails here.

Code grounding (commit as cloned):
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs lines 760-764: `json_segment(header, JwtSegment::Header)?` then `let _ = json_segment(claims, JwtSegment::Claims)?;` — the claims segment is required to decode to a JSON object before any header layer runs; `json_segment` (lines 824-833) returns TokenMalformation::NotJson(which) for a payload that is not a JSON object.
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs lines 423-437: `malformed_token` words Length | Segments | Encoding(_) | NotJson(_) | NotJoseHeader as "Failed to parse the token included in the request.", so NotJson(Claims) reaches the caller as that message.

Model grounding: AssumeRoleForPodIdentity errors = AccessDenied(400), ExpiredToken(400), InternalServer(500), InvalidParameter(400), InvalidRequest(400), InvalidToken(400), ResourceNotFound(404), ServiceUnavailable(503), Throttling(429). Nothing in the model constrains message text, so the "validation stops at kid resolution" boundary is recorded behavior only.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and observe eks-auth.pod-identity-token-claims-are-never-evaluated mismatch at its `len(distinct) == 1` assertion.
3. Reproduce directly. Fix one pattern-valid, unregistered clusterName (e.g. matching ^[0-9A-Za-z][A-Za-z0-9\-_]*$) and one header, base64url-encoded without padding: {"alg":"RS256","typ":"JWT","kid":"<44-character key id that no cluster's JWKS publishes>"}. Keep the signature segment constant (any base64url text).
4. Call AssumeRoleForPodIdentity five times, changing ONLY the payload segment: (a) a claim set inside its validity window; (b) the same claim set expired an hour ago; (c) the same claim set with aud=["sts.amazonaws.com"]; (d) base64url of {}; (e) base64url of bytes that are not JSON at all.
5. All five must answer HTTP 400, code InvalidTokenException, message 'The token included in the request has invalid header - kid.' — one byte-identical (code, message) identity. Observe that (e) instead answers 'Failed to parse the token included in the request.'
6. None of the five may answer ExpiredTokenException, AccessDeniedException, ResourceNotFoundException or InvalidRequestException: those are modeled on this operation but unreachable on real AWS, which never reads the claims.
7. Control that must keep working: send (a)'s payload with the header {"alg":"RS256","typ":"JWT"} (no `kid`); it must answer HTTP 400 InvalidTokenException 'The token included in the request has missing header - kid.', an identity distinct from the collapsed one — the header layer must still discriminate even though the claims layer must not.
8. Re-replay eks-auth.pod-identity-token-claims-are-never-evaluated; it must pass end to end before closing.
