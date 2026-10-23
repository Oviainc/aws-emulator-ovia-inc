Issue-Source: trace-sweep-divergence
Issue-Trace: eks-auth-assume-role-invalid-token-missing-kid
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-auth-assume-role-invalid-token-missing-kid eksauth-token-parse-and-header-rejections eks-auth-assume-role-invalid-token-pattern eksauth-constraint-validation-rejections
Issue-Shape-Id: com.amazonaws.eksauth#EKSAuthFrontend
Issue-Key: eks-auth|AssumeRoleForPodIdentity|4xx->4xx|$.message
Issue-Key: eks-auth|AssumeRoleForPodIdentity|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks-auth|AssumeRoleForPodIdentity|4xx->4xx|status
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# AssumeRoleForPodIdentity returns wrong rejection errors for invalid token and parameters

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleForPodIdentity

## Description
EKS Auth's only operation, AssumeRoleForPodIdentity (POST /clusters/{clusterName}/assume-role-for-pod-identity, restJson1, API version 2023-11-26), rejects bad input with the wrong modeled error throughout: the emulator either checks the wrong thing first or raises a generic framework-level ValidationException that this service does not model at all. The whole corpus for this service fails (9/9 steps mismatched, score 0%), clustered into the 3 causes below. All three are the same surface: the rejection path of AssumeRoleForPodIdentity does not produce the service's own error shapes with the service's own x-amzn-errortype value.

Cause 1 of 3 — a structurally-valid JWT missing the 'kid' header is answered with ResourceNotFoundException/404 instead of InvalidTokenException/400.
AWS: Smithy: AssumeRoleForPodIdentity errors include InvalidTokenException (httpError 400, doc "The specified Kubernetes service account token is invalid.") and ResourceNotFoundException (httpError 404). The recorded response for a token whose header lacks 'kid' is HTTP 400 with x-amzn-errortype 'InvalidTokenException:http://internal.amazon.com/coral/com.amazon.eksauth/' and message "The token included in the request has missing header - kid." (example text; the requirement is the 400 + InvalidTokenException). AWS therefore parses and validates the supplied token before it resolves the cluster named in the URI label.
Emulator: returns HTTP 404 with x-amzn-errortype 'ResourceNotFoundException' and message "The cluster <name> does not exist." — i.e. it resolves the clusterName label first and never reaches token parsing, so a malformed token is reported as a missing cluster.
Why: a pod-identity client (the EKS Pod Identity agent / SDK credential provider) distinguishes a retry-able/config error from a bad service-account token by error code and status. Getting 404 ResourceNotFoundException for a broken token sends it down the "cluster is wrong" path, so it never surfaces or refreshes the token, and 404 vs 400 also changes SDK retry/throw classification.
Fix: AssumeRoleForPodIdentity's handler/validation ordering — validate and parse the `token` input member (structure, required JWT headers) before the cluster lookup, and raise InvalidTokenException there.
Traces: eks-auth-assume-role-invalid-token-missing-kid#task#1 (representative); same cluster also covers eksauth-token-parse-and-header-rejections#task#1, #task#2, #task#3.

Cause 2 of 3 — a token failing the JwtToken pattern is answered with a generic ValidationException errortype, and the message echoes the sensitive token value.
AWS: Smithy: AssumeRoleForPodIdentityRequest.token targets JwtToken, @required, @length min 1, @pattern ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$, and is @sensitive; the service models InvalidParameterException (httpError 400, doc "The specified parameter is invalid. Review the available parameters for the API request.") and models no ValidationException at all. The recording answers HTTP 400 with x-amzn-errortype 'InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/' and a message built from the request that names the member and the constraint but does NOT include the offending value, because `token` is @sensitive: "1 validation error detected: Value at 'token' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+".
Emulator: status 400 is right, but x-amzn-errortype is 'ValidationException' (an error this service does not model, and without the coral shape-id suffix), and the message is "1 validation error detected: Value 'this-is-not-a-jwt-token' at 'token' failed to satisfy constraint: ..." — it interpolates the sensitive member's value. The generic constraint-trait validator is handling this member instead of the service's own validation.
Why: clients cannot catch the modeled InvalidParameterException (the SDK deserializes an unmodeled error type to a generic service exception), and a @sensitive credential-bearing member is echoed into an error body that gets logged, which is a credential-disclosure behavior AWS deliberately avoids.
Fix: AssumeRoleForPodIdentity's input validation / error serializer — map constraint failures on this service's members to InvalidParameterException with the eksauth coral errortype, and suppress the value of @sensitive members when composing the validation message.
Traces: eks-auth-assume-role-invalid-token-pattern#task#1 (representative); same cluster also covers eksauth-constraint-validation-rejections#task#1, #task#2, #task#4.

Cause 3 of 3 — the remaining constraint rejection also carries the generic ValidationException errortype instead of InvalidParameterException.
AWS: Smithy: the request's constrained members are clusterName (ClusterName, @required, @httpLabel, @length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) and token (above); every input-constraint rejection for this service is modeled as InvalidParameterException, httpError 400. The recording answers HTTP 400 with x-amzn-errortype 'InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/'.
Emulator: status 400 matches and the body matches, but x-amzn-errortype is 'ValidationException' (reported as a shape mismatch — wrong error shape, not merely different text).
Why: same client consequence as cause 2 — the thrown exception type is not the modeled one, so error handling keyed on InvalidParameterException never fires.
Fix: same component as cause 2 (AssumeRoleForPodIdentity error mapping/serializer); this cause confirms the mapping must be fixed for the clusterName/length constraint paths too, not just the token pattern path.
Traces: eksauth-constraint-validation-rejections#task#3.

No open issue covers any of this (no existing eks-auth / eksauth issue is on file). The replay reported no blocked steps, no stub, and state_reset_failures = 0, so these are genuine response-shape divergences rather than cross-fixture state bleed.

## Evidence
Replay of trace set simparity-wire-aws-v1: 4 fixtures, 9/9 steps mismatched, score_pct 0.0, bootstrap_signal false, emulator_up true, state_reset_failures 0, no stub_path and no owned_by on any cluster.

Cause 1 — fixture eks-auth-assume-role-invalid-token-missing-kid, step task#1 (affected_count 4):
  expected: HTTP 400, x-amzn-errortype 'InvalidTokenException:http://internal.amazon.com/coral/com.amazon.eksauth/', $.message 'The token included in the request has missing header - kid.'
  actual:   HTTP 404, x-amzn-errortype 'ResourceNotFoundException', $.message 'The cluster sim2real-probe-eks-auth-cluster-1ad61135 does not exist.'

Cause 2 — fixture eks-auth-assume-role-invalid-token-pattern, step task#1 (affected_count 4):
  expected: HTTP 400, x-amzn-errortype 'InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/', $.message "1 validation error detected: Value at 'token' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+" (no value echoed; token is @sensitive)
  actual:   HTTP 400, x-amzn-errortype 'ValidationException', $.message "1 validation error detected: Value 'this-is-not-a-jwt-token' at 'token' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$"

Cause 3 — fixture eksauth-constraint-validation-rejections, step task#3 (affected_count 1):
  expected: x-amzn-errortype 'InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/'
  actual:   x-amzn-errortype 'ValidationException' (verdict mismatch, shape_mismatch on the errortype header; status 400 agreed)

Model grounding (services/service-com-amazonaws-eksauth-eksauthfrontend/smithy-model/com-amazonaws-eksauth-eksauthfrontend.json): service EKSAuthFrontend, sdkId 'EKS Auth', endpointPrefix/arnNamespace 'eks-auth', protocol restJson1; AssumeRoleForPodIdentity errors = AccessDeniedException, ExpiredTokenException, InternalServerException, InvalidParameterException(400), InvalidRequestException, InvalidTokenException(400), ResourceNotFoundException(404), ServiceUnavailableException, ThrottlingException — ValidationException is NOT among them.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus); all 9 eks-auth steps mismatch.
3. Cause 1: replay fixture eks-auth-assume-role-invalid-token-missing-kid step task#1 — POST /clusters/<cluster>/assume-role-for-pod-identity with a syntactically valid three-part JWT whose header has no 'kid', against a cluster name that does not exist in emulator state. Observe HTTP 404 / x-amzn-errortype ResourceNotFoundException; AWS answers HTTP 400 / InvalidTokenException:http://internal.amazon.com/coral/com.amazon.eksauth/. (Also exercised by eksauth-token-parse-and-header-rejections task#1-#3.)
4. Cause 2: replay fixture eks-auth-assume-role-invalid-token-pattern step task#1 — same operation with token='this-is-not-a-jwt-token' (violates the JwtToken @pattern). Observe x-amzn-errortype ValidationException and the sensitive token value interpolated into $.message; AWS answers InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/ with the value omitted. (Also exercised by eksauth-constraint-validation-rejections task#1, #2, #4.)
5. Cause 3: replay fixture eksauth-constraint-validation-rejections step task#3 — another input-constraint rejection on the same operation. Observe x-amzn-errortype ValidationException instead of InvalidParameterException:http://internal.amazon.com/coral/com.amazon.eksauth/ (status 400 already matches).
6. After fixing, re-replay all of: eks-auth-assume-role-invalid-token-missing-kid#task#1, eks-auth-assume-role-invalid-token-pattern#task#1, eksauth-constraint-validation-rejections#task#3 — all three causes must pass before closing.
