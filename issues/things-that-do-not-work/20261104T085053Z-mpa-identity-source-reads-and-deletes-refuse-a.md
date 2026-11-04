Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-identity-source-lifecycle-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-identity-source-lifecycle-errors
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|GetIdentitySource|4xx->4xx|$.Message
Issue-Key: mpa|GetIdentitySource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|GetIdentitySource|4xx->4xx|status
Issue-Key: mpa|DeleteIdentitySource|4xx->4xx|$.Message
Issue-Key: mpa|DeleteIdentitySource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|DeleteIdentitySource|4xx->4xx|status
Issue-Commit: 2a533320acecc5e3d5510f43fe45b8adcca9ff2d

# mpa identity-source reads and deletes refuse a foreign-account ARN as 400 ValidationException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `2a533320acecc5e3d5510f43fe45b8adcca9ff2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIdentitySource

## Description
2 causes, one shared fault: the IdentitySourceArn acceptance check on the identity-source read and delete paths treats a well-formed ARN that the emulator's own account does not own as a malformed input, so it answers 400 ValidationException with the emulator's own wording instead of performing the authorization decision AWS makes and answering 403 AccessDeniedException. Both causes are reached with a syntactically valid MPA identity-source ARN naming a different account (arn:aws:mpa:<region>:<other-account>:identity-source/<name>).

Cause 1 of 2: GetIdentitySource with an identity-source ARN in another account (fixture mpa-identity-source-lifecycle-errors, step task#3).
AWS: Smithy: GetIdentitySource input IdentitySourceArn is @required, @httpLabel, String @length max 1000 — the recorded ARN satisfies that constraint, so it is not a validation failure. GetIdentitySource models AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action. Check your permissions, and try again."), ResourceNotFoundException (404) and ValidationException (400, doc "The input fails to satisfy the constraints specified by an Amazon Web Services service."). The recording shows AWS resolving a cross-account identity-source ARN as an authorization decision, not an input-shape decision: HTTP 403, x-amzn-errortype "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/", and Message with the standard IAM implicit-deny-on-resource-policy wording. That message's FIXED text is "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action"; its VARIABLE parts are the caller's principal ARN (from the signed request's credentials), the action name "mpa:GetIdentitySource" (the operation being invoked, repeated in the trailing clause) and the resource ARN taken verbatim from the request's IdentitySourceArn. The account id, region and identity-source name in the recorded example are request/environment values that fill that template, not part of it.
Emulator: HTTP 400, x-amzn-errortype "ValidationException", Message "Invalid IdentitySourceArn." — an emulator-authored string that is not AWS wording for any error class here.
Why: a client cannot tell "you lack permission on someone else's resource" from "you sent a malformed ARN". SDK error handling branches on the error type: AccessDeniedException is terminal-but-expected for cross-account probes and permission-boundary tests, whereas ValidationException tells the caller its own request is bad, so retry/permission-escalation logic and any catch of AccessDeniedException never fires, and code asserting the 403 status fails.
Fix: the GetIdentitySource handler's request validation / ARN resolution in the mpa service crate's generated per-operation handler (services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/, one file per operation): an ARN that parses and matches the identity-source form but is not owned by the emulator's account must leave validation and reach the authorization decision, which raises AccessDeniedException (403) with the IAM implicit-deny message above, rather than being rejected as an invalid ARN.
Traces: mpa-identity-source-lifecycle-errors#task#3.

Cause 2 of 2: DeleteIdentitySource with an identity-source ARN in another account (fixture mpa-identity-source-lifecycle-errors, step task#4).
AWS: Smithy: DeleteIdentitySource input IdentitySourceArn is @required, @httpLabel, String @length max 1000 (satisfied by the recorded ARN); its modeled errors are AccessDeniedException (403), ConflictException (409), ThrottlingException (429), InternalServerException (500) and ValidationException (400) — note it does NOT model ResourceNotFoundException, so an ARN the caller has no access to is answered as an authorization failure. The recording shows HTTP 403, x-amzn-errortype "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/", and the same implicit-deny Message template with the action "mpa:DeleteIdentitySource" and the resource ARN from the request.
Emulator: HTTP 400, x-amzn-errortype "ValidationException", Message "Invalid IdentitySourceArn.".
Why: a delete of a resource the caller cannot touch is reported as a client input error, so a caller that treats ValidationException as non-retryable-and-fatal-to-the-request shape (or that logs it as a bug in its own ARN construction) misdiagnoses a permissions problem; idempotent cleanup paths that tolerate 403 on foreign resources instead abort.
Fix: the DeleteIdentitySource handler's request validation / ARN resolution in the same per-operation handler directory — same ordering defect as Cause 1: authorization must be decided after ARN syntax, and a foreign-account ARN must yield AccessDeniedException (403), not ValidationException (400).
Traces: mpa-identity-source-lifecycle-errors#task#4.

Related but not covering this: open issue 20261024T191541Z-mpa-createapprovalteam-event-propagation.md concerns CreateApprovalTeam/CreateIdentitySource answering 403 because no wire path makes the caller the management account, and 20261028T100734Z-mpa-getresourcepolicy-wrong-behavior.md concerns GetResourcePolicy answering 404 for a stored identity source. Neither covers the read/delete ARN-acceptance ordering reported here.

## Evidence
Cause 1 — fixture mpa-identity-source-lifecycle-errors step task#3, GetIdentitySource:
  expected: status 403; x-amzn-errortype: "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/"; $.Message: "User: <caller-principal-arn> is not authorized to perform: mpa:GetIdentitySource on resource: <IdentitySourceArn from request> because no resource-based policy allows the mpa:GetIdentitySource action" (recorded instance: User: arn:aws:iam::000000000000:user/emulator ... resource: arn:aws:mpa:us-east-1:999999999999:identity-source/sim2real-probe-mpa-idsrc-12aa031e)
  actual:   status 400; x-amzn-errortype: "ValidationException"; $.Message: "Invalid IdentitySourceArn."

Cause 2 — fixture mpa-identity-source-lifecycle-errors step task#4, DeleteIdentitySource:
  expected: status 403; x-amzn-errortype: "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/"; $.Message: same template with action mpa:DeleteIdentitySource (recorded instance: User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: mpa:DeleteIdentitySource on resource: arn:aws:mpa:us-east-1:999999999999:identity-source/sim2real-probe-mpa-idsrc-12aa031e because no resource-based policy allows the mpa:DeleteIdentitySource action)
  actual:   status 400; x-amzn-errortype: "ValidationException"; $.Message: "Invalid IdentitySourceArn."

Model evidence (services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json): GetIdentitySource GET /identity-sources/{IdentitySourceArn}, errors AccessDeniedException(403)/InternalServerException(500)/ResourceNotFoundException(404)/ThrottlingException(429)/ValidationException(400); DeleteIdentitySource DELETE /identity-sources/{IdentitySourceArn}, errors AccessDeniedException(403)/ConflictException(409)/InternalServerException(500)/ThrottlingException(429)/ValidationException(400). IdentitySourceArn: String @length(max 1000) only — no pattern restricting the account id.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1): replay_wire_corpus().
2. Cause 1: fixture mpa-identity-source-lifecycle-errors, step task#3 — call mpa GetIdentitySource with a well-formed identity-source ARN whose account id is not the emulator's (e.g. GET /identity-sources/arn%3Aaws%3Ampa%3A<region>%3A999999999999%3Aidentity-source%2F<name>). Observe 400 ValidationException "Invalid IdentitySourceArn." where the recording has 403 AccessDeniedException with the IAM implicit-deny message naming mpa:GetIdentitySource and the requested ARN.
3. Cause 2: same fixture, step task#4 — call mpa DeleteIdentitySource with the same ARN (DELETE /identity-sources/{IdentitySourceArn}). Observe 400 ValidationException "Invalid IdentitySourceArn." where the recording has 403 AccessDeniedException naming mpa:DeleteIdentitySource.
4. Re-replay both steps after the fix; each must return 403 with x-amzn-errortype AccessDeniedException and the implicit-deny Message template populated from the caller principal, the operation's action name and the request's ARN.
