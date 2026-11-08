Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signer.access-denied-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer.access-denied-rejected|_poll_until#19|AssertionError|raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: 742d95e483c6290726dac54edb7cc16a76e76b19

# GetRevocationStatus is never authorized: an identity carrying an explicit Deny on signer:GetRevocationStatus is served HTTP 200 instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `742d95e483c6290726dac54edb7cc16a76e76b19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRevocationStatus

## Description
One net-new root cause on ONE operation of ONE service (GetRevocationStatus, GET /revocations, served by the signer crate's ops/get_revocation_status.rs). It is an authorization bypass on a security decision.

Cause 1 of 1: GetRevocationStatus never consults the caller's identity, so the modelled AccessDeniedException is unreachable and an explicitly-denied principal is served a success.
Failing check ids for this cause: signer.access-denied-rejected.

AWS: HTTP 403 with Error.Code 'AccessDeniedException'. Smithy (signer-data model com.amazonaws.signerdata#SignerDataPlane, operation GetRevocationStatus): errors include AccessDeniedException with errorKind 'client' and httpError 403, a structure with members {message, code}, documented "You do not have sufficient permissions to perform this action." The refusal message for an explicit identity-based Deny is worded: "User: <caller ARN> is not authorized to perform: signer:GetRevocationStatus on resource: <resource ARN> with an explicit deny in an identity-based policy". Fixed text: "User: " … " is not authorized to perform: signer:GetRevocationStatus on resource: " … " with an explicit deny in an identity-based policy". Variable parts, all taken from the request and from emulator state, never from any one recording: the caller's assumed-role session ARN, which has the shape arn:aws:sts::<accountId>:assumed-role/<roleName>/<sessionName> where accountId comes from the credential scope / sts.GetCallerIdentity, and roleName and sessionName come from the iam.CreateRole and sts.AssumeRole calls in the same run; and the resource ARN, which is the signing-profile ARN that the request's profileVersionArn names with its trailing version segment dropped (profileVersionArn arn:aws:signer:<region>:<account>:/signing-profiles/<name>/<version> yields resource arn:aws:signer:<region>:<account>:/signing-profiles/<name>). The modelled input members are unchanged between the allowed and the denied call (signatureTimestamp, platformId, profileVersionArn, jobArn, certificateHashes are all @required @httpQuery), so the refusal is driven by the caller's identity policy and by nothing else in the request. The one-sidedness matters: the same request from an unrestricted identity succeeds and returns a revokedEntities list, which the check verifies as a positive control before installing the Deny.

Emulator: the denied session is served exactly what the unrestricted session is served — HTTP 200 with a revokedEntities list — and no AccessDeniedException is ever produced. The check installs an inline {"Effect": "Deny", "Action": "signer:GetRevocationStatus", "Resource": "*"} on a freshly created role, assumes it, and then polls the identical GetRevocationStatus call for up to 90s waiting for Error.Code 'AccessDeniedException'; the denial never arrives and the poll ends with 'last saw (\'UNEXPECTED_SUCCESS\', None)' — the harness's sentinel for "the call returned 2xx". The handler reads only the request's region off the request context and never looks at the caller's identity or at any policy, so the AccessDeniedException variant the operation declares is declared and never constructed.

Why: a revocation check is a security decision, and the emulator answers it for a principal AWS refuses outright. A client that fences this data-plane read off with an explicit Deny — the strongest and least ambiguous IAM control there is — has that fence silently removed: least-privilege configurations validate against the emulator and then fail closed against AWS, which is the worst direction for this class of error. Negative-path tests that assert the 403 can never pass, and a caller that branches on AccessDeniedException to fall back to a cached or conservative answer instead takes the emulator's confident 'not revoked' and trusts a signature whose authorization was never evaluated.

Fix: an authorization gate for this operation in the signer crate, ahead of the handler body where the request's credential scope is already resolved — evaluate the caller's identity-based policy for action signer:GetRevocationStatus against the profile ARN derived from profileVersionArn, and on an explicit Deny answer the modelled AccessDeniedException (403) with the message template above. The role, its inline Deny and the assumed-role session are all created through the already-bootstrapped iam and sts services in the same run, so the policy to evaluate is present in state; the ARN-derivation helper and the message template belong beside the crate's other shared refusal wording so one rule words every operation's deny.

Related open issue, named as required rather than relied on: 20261029T230237Z-signer-data-getrevocationstatus-6-net-new-causes-certificatehashes.md describes this same behavior as its Cause 6 of 6, but it carries the service token 'signer-data' and its recorded replay key for this check is '_poll_until#19|Timeout|time.sleep(interval_s)' — a check-budget timeout. The suite now surfaces this check under the 'signer' service heading and it fails with an AssertionError out of _poll_until, so the replay key does not match and the suite did not mark this check as OWNED. This issue carries the current key under the service the check is actually reported against; whoever fixes either should close both together, as there is one underlying defect.

Traces: signer.access-denied-rejected.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 19/40 passed (47.5%), 13 mismatched, 8 errored, 0 skipped; the only service exercised was signer. signer.access-denied-rejected is the only failing check in the report that is neither marked 'ALREADY FILED' (blocked by the unimplemented SignPayload stub) nor marked 'OWNED by' an open issue.

[mismatch] signer.access-denied-rejected:
  File "check_signer_access_denied_rejected.py", line 81, in run
    code, message = _poll_until(
  File "check_signer_access_denied_rejected.py", line 119, in _poll_until
    raise AssertionError(f"{description} not observed within {timeout_s}s; last saw {last!r}")
  AssertionError: the deny policy on signer:GetRevocationStatus takes effect for the assumed role not observed within 90.0s; last saw ('UNEXPECTED_SUCCESS', None)

'UNEXPECTED_SUCCESS' is the check's own sentinel code, returned by its _client_error() helper when the wrapped call raises no botocore ClientError at all — i.e. the emulator answered 2xx. It is returned with message None because there was no error to read a message from. The check probed every 3.0s for 90.0s and saw that same success on every probe, so this is not a propagation-delay artifact.

Sequence actually exercised, all against emulated services:
  1. signer.GetRevocationStatus(platformId='Notation-OCI-SHA384-ECDSA', jobArn='arn:aws:signer:<region>:111122223333:/signing-jobs/'+'a'*20, profileVersionArn='arn:aws:signer:<region>:111122223333:/signing-profiles/n/'+'a'*10, certificateHashes=['a'*192], signatureTimestamp=now) with the default unrestricted identity -> succeeded and returned a list for revokedEntities. This positive control PASSED, so the operation itself is reachable and the divergence is confined to authorization.
  2. sts.GetCallerIdentity -> accountId.
  3. iam.CreateRole(RoleName=<generated>, AssumeRolePolicyDocument=trust policy allowing arn:aws:iam::<accountId>:root to sts:AssumeRole).
  4. iam.PutRolePolicy(PolicyName='deny-get-revocation-status', PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"signer:GetRevocationStatus","Resource":"*"}]}).
  5. sts.AssumeRole(RoleArn=<role>, RoleSessionName='sim2real-<8 hex>') -> session credentials.
  6. The IDENTICAL GetRevocationStatus from step 1, on that session.

Expected at step 6: HTTP 403, Error.Code 'AccessDeniedException', Error.Message "User: arn:aws:sts::<accountId>:assumed-role/<roleName>/<sessionName> is not authorized to perform: signer:GetRevocationStatus on resource: arn:aws:signer:<region>:111122223333:/signing-profiles/n with an explicit deny in an identity-based policy". The accountId, roleName, sessionName and region are per-run values filling that template — the requirement is the fixed text plus the derivation rule (session ARN from the assumed role, resource ARN = the request's profileVersionArn with its version segment dropped), not any one run's literals.
Actual at step 6: HTTP 200 with a revokedEntities list, no error, on every one of the ~30 probes across 90s.

Model facts used — smithy_query on services/service-com-amazonaws-signerdata-signerdataplane/smithy-model/com-amazonaws-signerdata-signerdataplane.json:
  - GetRevocationStatus: GET /revocations, readonly; errors AccessDeniedException(client, httpError 403), InternalServiceErrorException(server, 500), TooManyRequestsException(client, 429), ValidationException(client, 400).
  - AccessDeniedException = com.amazonaws.signerdata#AccessDeniedException, structure, errorKind client, httpError 403, members {message: String, code: String}, doc "You do not have sufficient permissions to perform this action."
  - Input members all @required @httpQuery: signatureTimestamp (Timestamp), platformId (PlatformId, @pattern ^[a-zA-Z0-9_-]+$), profileVersionArn (Arn, @length 20..2048), jobArn (Arn, @length 20..2048), certificateHashes (CertificateHashes list).
The exact deny-message wording is AWS's standard explicit-deny sentence for an identity-based policy; the model states the status, the error shape and its members, and the recording supplies the sentence.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite against it and filter to the 'signer' service heading.
3. Run check signer.access-denied-rejected. It performs, in order:
   a. signer.GetRevocationStatus with the default unrestricted test identity, using platformId='Notation-OCI-SHA384-ECDSA', jobArn='arn:aws:signer:<region>:111122223333:/signing-jobs/'+'a'*20, profileVersionArn='arn:aws:signer:<region>:111122223333:/signing-profiles/n/'+'a'*10, certificateHashes=['a'*192], signatureTimestamp=now. Confirm it returns 200 with a list for revokedEntities (positive control — this passes today and must keep passing).
   b. sts.GetCallerIdentity to read the account id.
   c. iam.CreateRole with a trust policy allowing arn:aws:iam::<accountId>:root to sts:AssumeRole.
   d. iam.PutRolePolicy installing the inline policy {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"signer:GetRevocationStatus","Resource":"*"}]}.
   e. sts.AssumeRole on that role to obtain session credentials.
   f. Repeat the byte-identical GetRevocationStatus from (a) using those session credentials, polling every 3s for up to 90s.
4. Observe at (f): HTTP 200 with a revokedEntities list on every probe, and the check failing with "the deny policy on signer:GetRevocationStatus takes effect for the assumed role not observed within 90.0s; last saw ('UNEXPECTED_SUCCESS', None)".
5. Expected at (f): HTTP 403, Error.Code 'AccessDeniedException', Error.Message "User: arn:aws:sts::<accountId>:assumed-role/<roleName>/<sessionName> is not authorized to perform: signer:GetRevocationStatus on resource: arn:aws:signer:<region>:111122223333:/signing-profiles/n with an explicit deny in an identity-based policy", where the session ARN is built from the role and session created in (c)/(e) and the resource ARN is the request's profileVersionArn with its trailing version segment dropped.
6. Regression guard: with the Deny removed (or from the unrestricted identity), the same call must still answer 200 with a revokedEntities list, so the gate must key off the caller's policy and not reject the operation outright.
