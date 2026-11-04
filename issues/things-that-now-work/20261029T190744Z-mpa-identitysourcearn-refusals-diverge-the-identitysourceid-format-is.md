Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mpa.identity-source-error-on-nonexistent-sso-instance
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa.identity-source-error-on-nonexistent-sso-instance|run#6|AssertionError|assert get_err == INVALID_ARN, get_err
Issue-Commit: 69bb2c4e0c47afe7ac06c67651974f0cf752730a

# mpa IdentitySourceArn refusals diverge: the IdentitySourceId format is never checked (404 ResourceNotFoundException instead of 400 ValidationException), and the malformed-ARN sentence reads "Invalid Resource Arn" instead of "Invalid IdentitySourceArn."

> _Note: this issue was identified against the code base at commit `69bb2c4e0c47afe7ac06c67651974f0cf752730a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIdentitySource

## Description
Two net-new root causes on the ARN-addressed identity-source reads (GetIdentitySource / DeleteIdentitySource). Both are grounded in the single failing check mpa.identity-source-error-on-nonexistent-sso-instance, and a fixer must close BOTH before that check can pass.

Cause 1 of 2: the resource portion of IdentitySourceArn is accepted in any shape, so an ARN that cannot name an identity source is looked up instead of refused.
AWS: GetIdentitySource/DeleteIdentitySource take IdentitySourceArn as an @httpLabel, @required String with length(max 1000) and NO @pattern (Smithy: GetIdentitySource.IdentitySourceArn -> String, httpLabel, required, length max 1000; same member on DeleteIdentitySource). Because the model fixes no pattern, the rule is recorded behavior only: AWS enforces the service's IdentitySourceId form on the resource portion of the ARN and refuses an otherwise well-formed mpa identity-source ARN whose id is not in that form (e.g. a plain UUID) with 400 ValidationException, Message "Invalid IdentitySourceArn.", on BOTH operations identically, BEFORE any existence check runs. The modeled error set confirms the ordering for the delete: DeleteIdentitySource declares AccessDeniedException(403), ConflictException(409), InternalServerException(500), ThrottlingException(429), ValidationException(400) and NO ResourceNotFoundException at all, so a nonexistent-but-plausible id cannot be answered as a lookup there; the recording shows the read taking the same ValidationException path rather than its ResourceNotFoundException. Message anatomy: the whole sentence "Invalid IdentitySourceArn." is FIXED text naming the MEMBER; no part of it is taken from the request, and the rejected ARN is NOT echoed.
Emulator: for arn:aws:mpa:us-east-1:<account>:identity-source/<uuid4> (caller's own account, correct partition/service/region/resource kind), GetIdentitySource answers 404 ResourceNotFoundException with Message "The requested resource does not exist." — the replay's observed pair was ('ResourceNotFoundException', 'The requested resource does not exist.') where ('ValidationException', 'Invalid IdentitySourceArn.') was recorded. The screen the handler applies (crate::state::is_identity_source_arn -> is_service_resource_arn) only asks for the ARN prefix/partition/service/region/account/resource-kind fields and accepts ANY non-empty id after "identity-source/", so the request falls through to the store lookup and the absent-record branch. The check asserts the identical ValidationException for DeleteIdentitySource on the same value; that assertion was not reached because the Get assertion failed first, but the delete handler resolves the same unchecked id and, finding no record, takes its @idempotent success path, so the strict-format refusal is unreachable on both operations.
Why: a client cannot distinguish "this is not a usable identity-source identifier" from "this identifier named a source that is gone". Code that branches on ValidationException to report a malformed input, and on ResourceNotFoundException to treat the resource as deleted, takes the delete branch for a value AWS never accepted; retry/backoff wrappers keyed on 404-vs-400 likewise invert. On the delete side it is worse: a caller is told a delete succeeded for an identifier AWS refuses outright.
Fix: the request validation of the two ARN-addressed identity-source handlers — the IdentitySourceArn screen must additionally require the service's IdentitySourceId form on the resource segment and refuse the malformed-ARN class there, before the account gate's sibling existence lookup; the predicate belongs beside the existing one in the state module's ARN helpers so both operations share it.
Traces: mpa.identity-source-error-on-nonexistent-sso-instance (assert get_err == ('ValidationException', 'Invalid IdentitySourceArn.')).

Cause 2 of 2: the malformed-ARN sentence for this member is the wrong text.
AWS: recorded behavior only (ValidationException$Message is @required and the model fixes no text): the refusal for a bad IdentitySourceArn is Message "Invalid IdentitySourceArn." — fixed text that NAMES the member. The code is ValidationException with HTTP 400.
Emulator: the service answers this whole class with one service-wide constant, crate::state::INVALID_ARN_MESSAGE = "Invalid Resource Arn" (returned by crate::state::invalid_arn()), which every ARN-bearing member of the crate shares. So even once Cause 1 routes the UUID-id ARN into invalid_arn(), the check still fails on the message: "Invalid Resource Arn" != "Invalid IdentitySourceArn." (different wording, and no trailing period).
Why: the message is part of the error contract. Clients and test suites that match the refusal text for this member see a sentence AWS never emits, and the member the request got wrong is not named, so the caller cannot tell which ARN-bearing member was rejected when several are present.
Fix: the serialization of the ValidationException these two handlers raise for IdentitySourceArn — the identity-source ARN class needs its own member-named sentence rather than the shared service-wide constant; keep it a single constant so Get and Delete cannot disagree.
Traces: mpa.identity-source-error-on-nonexistent-sso-instance (same assertion; the expected tuple pins both the code and the message).

Not covered by anything open or resolved: the resolved 20261027T180835Z-mpa-deleteidentitysource-error-response.md aligned DeleteIdentitySource's screen with GetIdentitySource's for WRONG-KIND ARNs (other resource kind, other namespace, bad partition/account) and explicitly pinned the shared "Invalid Resource Arn" text; it never touched the id FORMAT inside a correct-kind ARN, and it is the source of Cause 2's wording, not a fix for it. The open 20261028T150652Z-mpa-deleteidentitysource-wrong-behavior.md is the clock-side in-use rule, and 20261029T184914Z-mpa-session-reads-refuse-an-unknown-session-arn.md is the SESSION read's 404-vs-403 class, a different resource and a different expected code. The positive control in the same check (ListIdentitySources with MaxResults=20 returning an IdentitySources page) passes, so only the resource-addressed refusal path diverges.

## Evidence
Failing check: mpa.identity-source-error-on-nonexistent-sso-instance (group overly-permissive; tags axis:rejection, cap:c1, concern:negative-classes, plane:control).

Replay output:
  File "check_mpa_identity_source_error_on_nonexistent_sso_instance.py", line 33, in run
    assert get_err == INVALID_ARN, get_err
  AssertionError: ('ResourceNotFoundException', 'The requested resource does not exist.')
where the check's recorded expectation is
  INVALID_ARN = ("ValidationException", "Invalid IdentitySourceArn.")

Expected (AWS, both operations, before any existence check):
  HTTP 400, Code ValidationException, Message "Invalid IdentitySourceArn."
Actual (emulator, GetIdentitySource):
  HTTP 404, Code ResourceNotFoundException, Message "The requested resource does not exist."
Actual (emulator, DeleteIdentitySource — same unchecked id, assertion not reached):
  the @idempotent no-record success path (200, empty body); DeleteIdentitySource models no ResourceNotFoundException, so no refusal exists on that path at all.

Model, for the ordering and the absent 404 on the delete (smithy_query query="operation", model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json"):
  GetIdentitySource: input {IdentitySourceArn: String, httpLabel, required, length max 1000}; errors AccessDenied(403), InternalServer(500), ResourceNotFound(404), Throttling(429), Validation(400); GET /identity-sources/{IdentitySourceArn}
  DeleteIdentitySource: same input member; errors AccessDenied(403), Conflict(409), InternalServer(500), Throttling(429), Validation(400) — no ResourceNotFoundException; DELETE /identity-sources/{IdentitySourceArn}, @idempotent

Emulator code the two causes live in:
  services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_identity_source.rs:208-213 — the only screen on the member is `if !crate::state::is_identity_source_arn(&input.identity_source_arn) { ... invalid_arn() ... }`
  same file:237-245 — anything that passes that screen falls through to `state.identity_source(..)` and, when absent, to `ResourceNotFoundException(crate::state::resource_not_found())`
  services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs:2184-2194 — `is_identity_source_arn` = `is_service_resource_arn(arn, IDENTITY_SOURCE_RESOURCE)`
  same file:3608-3630 — `is_service_resource_arn` validates prefix/partition/service/region/account and the resource KIND only; the id after `identity-source/` is unconstrained (contrast the minting side at state.rs:2176-2182, which writes `identity-source/ids-{seq:012}`)
  same file:3760-3778 — `INVALID_ARN_MESSAGE = "Invalid Resource Arn"` and `invalid_arn()`, the one sentence the crate answers the whole class with (Cause 2)
  services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/delete_identity_source.rs — same member, same screen after the resolved 20261027 issue, then the no-record success return

## Steps to Reproduce
1. Boot the emulator and point a botocore mpa client at it in us-east-1 (MPA is single-Region: the check builds ctx.client("mpa", region="us-east-1")).
2. Resolve the caller's account with sts.get_caller_identity()["Account"].
3. Build an ARN that is a well-formed mpa identity-source ARN in that account but whose id is not the service's IdentitySourceId form:
   arn:aws:mpa:us-east-1:<account>:identity-source/<uuid.uuid4()>
4. Call GetIdentitySource(IdentitySourceArn=<that ARN>).
   Expected: HTTP 400 ValidationException, Message "Invalid IdentitySourceArn."
   Actual:   HTTP 404 ResourceNotFoundException, Message "The requested resource does not exist."
5. Call DeleteIdentitySource(IdentitySourceArn=<the same ARN>).
   Expected: the identical HTTP 400 ValidationException, Message "Invalid IdentitySourceArn."
   Actual:   the @idempotent success (HTTP 200, empty body) — no refusal at all.
6. Positive control (passes today, must keep passing): ListIdentitySources(MaxResults=20) returns a page carrying IdentitySources.
7. Re-replay mpa.identity-source-error-on-nonexistent-sso-instance to confirm; note that fixing only the error class (Cause 1) still fails the same assertion on the message, because the crate's shared sentence is "Invalid Resource Arn" (Cause 2).
