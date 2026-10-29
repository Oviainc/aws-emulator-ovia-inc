Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks-auth.pod-identity-header-rejection-layers-are-ordered
Issue-Shape-Id: com.amazonaws.eksauth#EKSAuthFrontend
Issue-Key: eks-auth.pod-identity-header-rejection-layers-are-ordered|run#13|AssertionError|assert layer1 == ("InvalidTokenException", _PARSE), (
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# eks-auth AssumeRoleForPodIdentity assigns token-header rejections to the wrong layer (missing `alg` reported as a missing-header instead of a parse failure)

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleForPodIdentity

## Description
REOPENS 20261023T055957Z-eks-auth-assumeroleforpodidentity-returns-wrong-rejection-errors-for-invalid-token.md: its fix left the call refused. That fixed issue covered this operation's token parse/header rejection path (its Cause 1 and its trace `eksauth-token-parse-and-header-rejections`, steps task#1-#3) and did move the refusal to HTTP 400 / InvalidTokenException ahead of the cluster lookup — the behavioral check now clears the constraint layer (layer 0 of its assertion passes). The call is still refused with the wrong identity, however: the token-header rejection that follows reports a different InvalidTokenException *message* than AWS, because the emulator's JOSE-header parse assigns the rejection to the wrong layer. The still-open issue 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md lists this check id but carries it under a replay key for the layer-0 (ValidationException vs InvalidParameterException) assertion, which now passes; the layer-1 divergence below is not described there or in any other open issue.

Cause 1 of 1: a JOSE header that omits the `alg` parameter is reported as a missing-header rejection instead of a parse failure, so the five ordered token-header rejection layers are mis-assigned.

AWS: Smithy (services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json): AssumeRoleForPodIdentity (POST /clusters/{clusterName}/assume-role-for-pod-identity, restJson1) models InvalidTokenException with httpError 400, documented "The specified Kubernetes service account token is invalid."; input `token` targets JwtToken, @required, @sensitive, @length min 1, @pattern ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$. The model says nothing about WHICH message belongs to which header defect, so the layer assignment is recorded behavior only (measured against real eks-auth in us-west-2 by this check). Recorded behavior: every token-header rejection is HTTP 400 / InvalidTokenException, and the message identifies the shallowest layer the token fails at, in this order:
  1. structural parse — the header segment must base64url-decode to a JSON object whose `alg` is PRESENT and a string (and whose `kid`, if present, is a string). A header with no `alg` at all does not parse, so the message is the fixed text "Failed to parse the token included in the request." (no variable part).
  2. the `alg` VALUE must be exactly "RS256" -> "The token included in the request has invalid header - alg." (fixed text; `alg` is the member name, not a request value).
  3. `kid` must be present and non-blank, where empty string, whitespace-only and JSON null all count as ABSENT -> "The token included in the request has missing header - kid."
  4. `kid` must resolve against the named cluster's OIDC JWKS -> "The token included in the request has invalid header - kid."
The shallowest failing layer wins, so a request that is wrong at several layers reports only the shallowest. In every one of these messages the only variable part is the header member name (`alg`/`kid`); the rest is fixed text, and no part of the @sensitive token value appears.

Emulator: a header `{"kid": ""}` (no `alg`, blank `kid`) against a pattern-valid cluster name is answered HTTP 400 ('InvalidTokenException', 'The token included in the request has missing header - alg.') — a message AWS does not emit for this class at all. AWS reports ('InvalidTokenException', 'Failed to parse the token included in the request.'). The code grounds it: services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs `parse_service_account_token` finishes the structural parse first and then loops REQUIRED_JOSE_HEADERS = ["alg", "kid"], returning TokenMalformation::MissingHeader("alg") for an absent `alg`; services/.../src/ops/assume_role_for_pod_identity.rs `malformed_token` words MissingHeader(name) as "...has missing header - {name}." and reserves "Failed to parse the token included in the request." for the Length/Segments/Encoding/NotJson variants only. The same mis-assignment affects the two layers beneath, which this check reaches next: (a) the `alg` VALUE is never judged — `header_value_is_usable` in state.rs returns true for every `alg`, so "RS256" is not required and the "...has invalid header - alg." message is unreachable; (b) a blank `kid` is filtered by `string_header` and reported as TokenMalformation::InvalidHeader("kid") -> "...has invalid header - kid.", whereas AWS treats blank as ABSENT and reports "...has missing header - kid.". Those two are code-grounded; only (the absent-`alg`) layer is directly observed, because the check's assertion stops there.

Why: the EKS Pod Identity Agent and the SDK credential provider surface this message to the operator as the reason a pod could not get credentials, and operators and tests match on it to tell "the projected token is not a JWT" from "the cluster's signing key rotated". Reporting an unparseable header as a missing `alg` points the operator at a header field that is not the problem; and because the layers collapse, two tokens that are wrong in different ways become indistinguishable — the check also asserts the four token layers produce four distinct identities, which the emulator cannot satisfy while `alg`'s value is unchecked and blank `kid` is folded onto the invalid-`kid` message. HTTP status and error code are correct, so only the message/diagnosis diverges.

Fix: this operation's token parsing and its error wording — `parse_service_account_token` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs must treat an absent-or-non-string `alg` as a parse failure (the TokenMalformation variant `malformed_token` words as "Failed to parse the token included in the request."), must require the `alg` value to be exactly RS256 as its own layer ahead of any `kid` check, and must classify a blank/whitespace/null `kid` as MissingHeader("kid") rather than InvalidHeader("kid"), leaving InvalidHeader("kid") for a non-blank kid that does not resolve against the cluster's keys; `malformed_token` in services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs keeps the wording but must receive the right variant. No part of the @sensitive token may enter any of these messages (that already holds).

Traces: eks-auth.pod-identity-header-rejection-layers-are-ordered.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 6/22 passed; eks-auth 6/21 (15 failed), eks__eks-auth 0/1 (1 failed). Of the failures, 11 are ALREADY FILED (blocked by unimplemented EC2 CreateInternetGateway) and 4 are OWNED by 20261023T153145Z-eks-auth-assumeroleforpodidentity-rejects-in-the-wrong-order.md. The single remaining failure is the one filed here.

[mismatch] eks-auth.pod-identity-header-rejection-layers-are-ordered, at check_eks_auth_pod_identity_header_rejection_layers_are_ordered.py line 113:
  assert layer1 == ("InvalidTokenException", _PARSE)
  AssertionError: a header with no `alg` and a blank `kid` returned ('InvalidTokenException', 'The token included in the request has missing header - alg.'); a header missing `alg` does not parse, so real AWS reports ('InvalidTokenException', 'Failed to parse the token included in the request.') and never reaches the kid layer

Request that produced it: clusterName='sim2realbehavabsentcluster' (pattern-valid, not registered), token = base64url({"kid": ""}) + '.' + base64url({"aud":["pods.eks.amazonaws.com"],"iat":<now>,"exp":<now+3600>,"sub":"system:serviceaccount:default:sim2real-behav-sa"}) + '.c2lnbmF0dXJl'.
  expected: HTTP 400, InvalidTokenException, message 'Failed to parse the token included in the request.'
  actual:   HTTP 400, InvalidTokenException, message 'The token included in the request has missing header - alg.'

Progress relative to the fixed issue: the same check's layer-0 assertion (a pattern-invalid clusterName '-sim2realbehav' must yield InvalidParameterException with the unanchored pattern in the message) now PASSES, so the constraint-error mapping from 20261023T055957Z landed; the refusal that follows it is still wrong.

Code grounding (commit as cloned):
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs lines 698-741: `parse_service_account_token` completes the structural parse (segments, base64url, JSON-object) and only then loops REQUIRED_JOSE_HEADERS (line 87: ["alg", "kid"]), returning `TokenMalformation::MissingHeader(name)` at line 727 when the member is absent.
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs lines 750-755: `header_value_is_usable` matches only "kid"; any `alg` string is accepted, so RS256 is never required.
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/state.rs lines 760-768: `string_header` filters blank values, so `kid: ""` falls into the `InvalidHeader("kid")` arm at line 732 instead of the missing-header arm.
- services/service-com-amazonaws-eksauth-eksauthfrontend/src/ops/assume_role_for_pod_identity.rs lines 418-431: `malformed_token` words MissingHeader/InvalidHeader as "...has missing/invalid header - {name}." and emits "Failed to parse the token included in the request." only for Length | Segments | Encoding | NotJson.

Model grounding: AssumeRoleForPodIdentity errors = AccessDenied(400), ExpiredToken(400), InternalServer(500), InvalidParameter(400), InvalidRequest(400), InvalidToken(400), ResourceNotFound(404), ServiceUnavailable(503), Throttling(429). The model constrains nothing about message text, so the layer-to-message mapping above is recorded behavior only.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and observe eks-auth.pod-identity-header-rejection-layers-are-ordered mismatch at its layer-1 assertion.
3. Reproduce directly: call eks-auth AssumeRoleForPodIdentity with clusterName='sim2realbehavabsentcluster' (matches ^[0-9A-Za-z][A-Za-z0-9\-_]*$, not registered) and token='<base64url of {"kid": ""}>.<base64url of any JSON claim object>.c2lnbmF0dXJl'. Observe HTTP 400 InvalidTokenException with message 'The token included in the request has missing header - alg.'; AWS answers HTTP 400 InvalidTokenException with 'Failed to parse the token included in the request.'
4. Same cluster name, header {"alg": "HS256", "kid": ""}: AWS answers 'The token included in the request has invalid header - alg.' (only RS256 is accepted). Verify the emulator does not accept a non-RS256 alg and does not report the kid layer here.
5. Same cluster name, header {"alg": "RS256", "kid": ""}: AWS treats the blank kid as ABSENT and answers 'The token included in the request has missing header - kid.' Verify the emulator does not answer '...has invalid header - kid.'
6. Same cluster name, header {"alg": "RS256", "typ": "JWT", "kid": "sim2realbehaveksauthKid00000000000000000000"} (non-blank, unresolvable): AWS reaches kid resolution and answers 'The token included in the request has invalid header - kid.'
7. All four token-header rejections must be HTTP 400 / InvalidTokenException and must produce four DISTINCT messages; a request wrong at several layers must report only the shallowest.
8. Re-replay eks-auth.pod-identity-header-rejection-layers-are-ordered; it must pass end to end before closing.
