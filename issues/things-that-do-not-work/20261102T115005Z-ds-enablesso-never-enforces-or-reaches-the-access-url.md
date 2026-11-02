Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.enable-sso-without-prerequisites-denied
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.enable-sso-without-prerequisites-denied|run#12|AssertionError|assert code == "ClientException", f"expected ClientException, got {code}"
Issue-Commit: 406bd2208b15ed2c2c5b06aef0a099e8d15489d9

# EnableSso never enforces (or reaches) the access-URL/alias prerequisite: it answers 501 NotImplemented instead of 400 ClientException, and SSO can never be turned on

> _Note: this issue was identified against the code base at commit `406bd2208b15ed2c2c5b06aef0a099e8d15489d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableSso

## Description
One net-new root cause (Cause 1 of 1), grounded in failing check `ds.enable-sso-without-prerequisites-denied`. This check was NOT marked ALREADY FILED or OWNED by the replay report, so it is filed here.

Dedupe note for the fixer: two open issues touch this operation but neither carries this check's replay key as the report resolves it — `20261029T200700Z-ds-enablesso-stub.md` ("Implement ds EnableSso: unimplemented scaffold stub", which scopes itself to "implement the operation" and names only the blocked unit `ds.disable-sso-authentication-failed-rejected`), and `20261029T214911Z-ds-enablesso-has-no-missing-prerequisite-refusal-enabling-sso.md`, whose Issue-Key pins an older source line of the check file (`run#11`) and therefore no longer matches the replayed failure (now line 47). If those are closed as part of the same change, re-replay the check id below to confirm; this issue records the contract with the exact refusal message the replay asserts, which the earlier issue left hedged.

Cause 1 of 1: EnableSso is a scaffold that answers 501 for every request, so neither the missing-access-URL refusal nor the success path exists. Failing check: ds.enable-sso-without-prerequisites-denied.

AWS: Smithy (com.amazonaws.directoryservice#DirectoryService_20150416) models EnableSso with input `DirectoryId` (@required, @pattern ^d-[0-9a-f]{10}$), optional `UserName` (@pattern ^[a-zA-Z0-9._-]+$) and `Password` (@length 1..128, @sensitive); output `EnableSsoResult` is empty with SUCCESS_STATUS 200; the only modeled errors are AuthenticationFailedException, ClientException, EntityDoesNotExistException, InsufficientPermissionsException (client, httpError 400) and ServiceException (server, httpError 500). `NotImplemented` is not among them. AWS requires an access URL (alias) before single sign-on can be enabled; per API_DirectoryDescription.html, "If no alias exists, the alias is the directory identifier", so a directory that has never had CreateAlias called is observable as `Alias == DirectoryId` with `SsoEnabled == false`. On such a directory AWS refuses EnableSso with HTTP 400, Error.Code `ClientException`, and (recorded behavior only, as asserted by the replayed check) the message `An alias is required before enabling SSO. DomainId=<directory-id> : RequestId: <request-id>`. Fixed text of that message: `An alias is required before enabling SSO. DomainId=` and the ds error family's trailing ` : RequestId: ` suffix. Variable parts: `<directory-id>` is taken from the request's DirectoryId, `<request-id>` from the response metadata of that same call — neither the id nor the request id from any one recording is part of the requirement. The refusal must have no effect: DescribeDirectories still reports `SsoEnabled == false`. After the single missing prerequisite is supplied — CreateAlias(DirectoryId, Alias) (@required Alias, @length 1..62, @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$), after which DescribeDirectories reports `Alias == <alias>` — the identical EnableSso call must SUCCEED with 200 and an empty body, and DescribeDirectories must then report `SsoEnabled == true`.

Emulator: the EnableSso route returns HTTP 501 with Error.Code `NotImplemented` and the scaffold message "operation EnableSso is scaffolded but not implemented in this emulator" for every request. The replay captured `AssertionError: expected ClientException, got NotImplemented`. Because the operation never answers, the pre-alias refusal, its no-effect guarantee, the post-CreateAlias success and the `SsoEnabled == true` projection are all unreachable, and no SSO state is persisted at all.

Why: a client that catches ClientException to tell the operator "create an access URL first" instead receives an unmodeled 501 `NotImplemented`, which SDK error classification treats as a server-side/unknown failure rather than a caller-fixable precondition — so the operator is never told what to fix, and retry logic may retry a call that can never succeed. Symmetrically, a client that enables SSO as part of provisioning gets a hard failure where AWS turns SSO on, and a client reading back `SsoEnabled` can never observe it become true. Once implemented, the one outcome that must stay impossible is SSO being silently enabled with no access URL: the refusal must leave `SsoEnabled` false.

Fix: the operation's handler and its validation in the directoryservice crate, plus the directory state that holds the SSO flag and alias so DescribeDirectories can project `SsoEnabled` and the alias-defaults-to-directory-id rule. The alias-present precondition belongs in that handler's validation ahead of any state mutation, and its refusal must be classified as the modeled ClientException (400), not the scaffold's NotImplemented (501); delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker once implemented.

Traces: ds.enable-sso-without-prerequisites-denied.

## Evidence
Failing check: [mismatch] ds.enable-sso-without-prerequisites-denied

Replay output:
  File "check_ds_enable_sso_without_prerequisites_denied.py", line 47, in run
    assert code == "ClientException", f"expected ClientException, got {code}"
  AssertionError: expected ClientException, got NotImplemented

Expected (AWS): EnableSso(DirectoryId=<d-...>) on an Active Microsoft AD that has never had CreateAlias called -> HTTP 400, Error.Code = "ClientException", Error.Message = "An alias is required before enabling SSO. DomainId=<directory-id> : RequestId: <request-id>" (fixed text as quoted; <directory-id> from the request, <request-id> from the response metadata). DescribeDirectories afterwards still reports SsoEnabled = false. Then CreateAlias(DirectoryId, Alias=<alias>) -> DescribeDirectories reports Alias = <alias>; the identical EnableSso call -> HTTP 200 with an empty EnableSsoResult body; DescribeDirectories then reports SsoEnabled = true.

Actual (emulator): EnableSso -> HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation EnableSso is scaffolded but not implemented in this emulator", on both the pre-alias refusal and the post-CreateAlias success path. No SSO state is persisted, so SsoEnabled never becomes true.

Handler citation: services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/enable_sso.rs lines 3-6 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker, and line 167 of `EnableSso::handle` is `Err(crate::scaffolded::Scaffolded::not_implemented("EnableSso"))` — the only statement in the handler body. Lines 96-102 show the modeled error enum already offers ClientException.

Smithy citation: EnableSso input DirectoryId @required @pattern ^d-[0-9a-f]{10}$, UserName @pattern ^[a-zA-Z0-9._-]+$, Password @length 1..128 @sensitive; output EnableSsoResult (empty), SUCCESS_STATUS 200; errors AuthenticationFailedException, ClientException, EntityDoesNotExistException, InsufficientPermissionsException (client, httpError 400), ServiceException (server, httpError 500). CreateAlias input Alias @required @length 1..62 @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$; output {DirectoryId, Alias}.

## Steps to Reproduce
1. Boot the ds (Directory Service) emulator.
2. Create a VPC with two subnets in two distinct availability zones (ec2 CreateVpc + CreateSubnet).
3. ds CreateMicrosoftAD(Name=<label>.example.com, Password=<compliant password>, Edition="Standard", VpcSettings={VpcId, SubnetIds=[the two subnets]}) and wait for DescribeDirectories to report Stage = "Active".
4. ds DescribeDirectories(DirectoryIds=[<directory-id>]) and confirm the baseline: SsoEnabled = false and Alias = <directory-id> (no access URL yet).
5. ds EnableSso(DirectoryId=<directory-id>) with no alias created. Observe HTTP 501 / Error.Code "NotImplemented"; AWS answers HTTP 400 / "ClientException" with "An alias is required before enabling SSO. DomainId=<directory-id> : RequestId: <request-id>".
6. ds DescribeDirectories again and confirm SsoEnabled is still false (the refusal must have no effect).
7. Positive control: ds CreateAlias(DirectoryId=<directory-id>, Alias=<alias>), confirm DescribeDirectories reports Alias = <alias>, then repeat the identical ds EnableSso call. It must return HTTP 200 with an empty body and DescribeDirectories must then report SsoEnabled = true; the emulator returns 501 NotImplemented here too.
