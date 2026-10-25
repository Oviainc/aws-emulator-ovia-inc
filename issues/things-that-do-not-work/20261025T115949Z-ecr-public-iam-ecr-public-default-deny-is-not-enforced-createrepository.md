Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr-public__iam.iam-policy-without-create-repository-blocks-only-that-action
Issue-Key: ecr-public__iam.iam-policy-without-create-repository-blocks-only-that-action|_error_message_and_code#16|AssertionError|raise AssertionError(f"{what} succeeded; it must have been refused")
Issue-Commit: 69d9d2e12a458c911688cbf85bce3839498ad470

# ECR Public: default-deny is not enforced — CreateRepository succeeds for an IAM principal whose identity policy does not Allow ecr-public:CreateRepository (1 cause)

> _Note: this issue was identified against the code base at commit `69d9d2e12a458c911688cbf85bce3839498ad470`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRepository

## Description
=== Cause 1 of 1: an action an IAM identity policy does not Allow is performed anyway; ecr-public applies no default deny ===

RELATION TO ALREADY-OPEN WORK: the open issue 20261023T055032Z-ecr-public-iam-an-action-scoped-iam-identity.md is grounded in this same check, but in its PREVIOUS failure signature — the check was cut off at "Check timed out after 60.0s" inside a non-converging poll, i.e. the emulator did not bind an IAM-minted access key to its user and the scoped-Allow positive control never became true. That part now works: in this replay the key-activation poll converged (sts:GetCallerIdentity signed with the scoped key returned the created user's ARN) and BEAT 1 completed, so the check now reaches BEAT 2 and fails there with a DIFFERENT, net-new signature — an unexpected SUCCESS, not a timeout. The replay key therefore no longer matches that issue and the suite did not mark this check OWNED. The companion open issue 20261023T055032Z-ecr-public-getauthorizationtoken-is-not-authorization-gated-stored.md covers the same gap on GetAuthorizationToken only; neither issue covers CreateRepository's authorization decision. This is an unexpected success, not a refusal, so it is not a reopen of any fixed issue.

AWS: Smithy ecr-public CreateRepository (input repositoryName required, @length 2..205, pattern ^(?:[a-z0-9]+(?:[._-][a-z0-9]+)*/)*[a-z0-9]+(?:[._-][a-z0-9]+)*$; output repository + catalogData) models InvalidParameterException, InvalidTagParameterException, LimitExceededException, RepositoryAlreadyExistsException, TooManyTagsException, UnsupportedCommandException (all client/400) and ServerException (500). AccessDeniedException is deliberately NOT in that modeled set: authorization is the service-common IAM decision taken before the operation runs, so the model says nothing about it and the requirement is IAM's own rule. That rule is default deny: an action that no identity-based (or resource-based) policy statement Allows is refused, and the refusal is the AccessDeniedException client error. So for an IAM user whose ONLY inline policy Allows ecr-public:DescribeRepositories, ecr-public:GetAuthorizationToken and sts:GetServiceBearerToken (Resource "*"), a CreateRepository signed with that user's access key must be refused AccessDeniedException, no repository may come into existence (an administrative DescribeRepositories for the name must answer RepositoryNotFoundException), and after ecr-public:CreateRepository is added to that same inline policy the IDENTICAL call must succeed. The denial message follows IAM's standard explicit-template wording; the check pins it as:
  User: <caller-arn> is not authorized to perform: <action> on resource: <resource-arn> because no identity-based policy allows the <action> action
Fixed text (the requirement): "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action". Variable parts, taken from the request and the emulator's own state, never from any one recording: <caller-arn> is the signing principal's ARN as sts:GetCallerIdentity reports it for that access key; <action> is the ecr-public action being authorized, here ecr-public:CreateRepository; <resource-arn> is arn:aws:ecr-public::<account-id>:repository/<repositoryName>, with <account-id> the caller's account and <repositoryName> the value sent in the request. The randomly generated user name and repository name in any single run fill that template and are not part of it.

Emulator: the call is performed. CreateRepository signed with the scoped principal's key returns 2xx and a repository, and the check fails at BEAT 2 with
  AssertionError: CreateRepository as sim2real-behav-ecrpub-iam-<id>, whose inline policy Allows only ['ecr-public:DescribeRepositories', 'ecr-public:GetAuthorizationToken'] succeeded; it must have been refused
raised by the harness helper _error_message_and_code, which exists precisely to turn an unexpected success into a failure. No AccessDeniedException is produced at all, so the asserted code and the asserted message are both unreachable. BEAT 1 (the positive control: the scoped principal's DescribeRepositories and GetAuthorizationToken succeed) "passes", but on this emulator it passes vacuously — it succeeds because every call from every credential is permitted, not because the Allow was honoured. The emulator now resolves the access key to the right principal (the ARN poll converged) but then never evaluates that principal's policy against the action, i.e. the authorization step is effectively allow-all rather than default-deny.

Why: this is the least-privilege path. A caller validating that a narrowed IAM policy blocks exactly one ecr-public action — the normal way an operator proves a role is correctly scoped before shipping it — gets a green result from the emulator for a policy that AWS rejects. Worse, the failure is in the unsafe direction: a credential that AWS refuses is allowed to create a world-visible public ECR repository, and the created side effect is real in emulator state, so negative tests of over-broad or missing grants pass locally and only fail (or, in production, only take effect) against real AWS. It also makes the positive control meaningless: because nothing is denied, an emulator run cannot distinguish "this action was denied" from "this principal has no permissions at all".

Fix: the request-authorization step shared by the ecr-public operation handlers — the pre-dispatch authorizer, not CreateRepository's input validation. Having resolved the SigV4 access key to the IAM user that iam:CreateAccessKey minted, it must evaluate that user's inline and attached policies for the ecr-public action mapped to the operation being dispatched, and when no statement Allows it, short-circuit with the AccessDeniedException client error carrying the message template above, built from the resolved caller ARN, the mapped action and the request's repositoryName — never running the handler, so no repository is created. The same authorizer must govern every ecr-public operation, so the Allow-side positive control (DescribeRepositories, GetAuthorizationToken) is honoured because the policy permits it rather than because nothing is checked.

Traces: ecr-public__iam.iam-policy-without-create-repository-blocks-only-that-action.

## Evidence
Replay: simparity-behavioral-ga-prod-v1. Totals 61/75 (81.3%); suite ecr-public__iam 0/1 (0%), 1 failed.

Failing check: [mismatch] ecr-public__iam.iam-policy-without-create-repository-blocks-only-that-action
  File "check_ecr_public__iam_iam_policy_without_create_repository_blocks_only_that_action.py", line 248, in run
    code, message = _error_message_and_code(_scoped_create, ...)
  File "...", line 104, in _error_message_and_code
    raise AssertionError(f"{what} succeeded; it must have been refused")
  AssertionError: CreateRepository as sim2real-behav-ecrpub-iam-c9a0d05bec5d, whose inline policy Allows only ['ecr-public:DescribeRepositories', 'ecr-public:GetAuthorizationToken'] succeeded; it must have been refused

How far the check got, which localises the defect precisely:
- _scoped_principal: iam:CreateUser, iam:PutUserPolicy (inline Allow of ecr-public:DescribeRepositories, ecr-public:GetAuthorizationToken, sts:GetServiceBearerToken on Resource "*"), iam:CreateAccessKey — all succeeded.
- H4 key-activation poll: converged; sts:GetCallerIdentity signed with the scoped key returned the created user's ARN (the assert at line ~1646 of the check did not fire). The key IS bound to the scoped principal, so the identity is available to an authorizer.
- BEAT 1 positive control: the scoped principal's ecr-public DescribeRepositories was permitted and GetAuthorizationToken returned a non-empty authorizationToken. Both passed.
- BEAT 2: ecr-public CreateRepository signed with the SAME key returned success. Expected AccessDeniedException; got a 2xx repository. The two asserts that follow (code == "AccessDeniedException", and the exact IAM-template message naming ecr-public:CreateRepository) were never reached, so the error code and message are unverified only because no error was produced.
- BEATs 3-6 (effect-absent read, restore-grant, identical-call-succeeds, terminal admin read) were not reached.

Expected vs actual at BEAT 2:
  expected: AccessDeniedException, client error, message "User: <scoped-user-arn> is not authorized to perform: ecr-public:CreateRepository on resource: arn:aws:ecr-public::<account-id>:repository/<repositoryName> because no identity-based policy allows the ecr-public:CreateRepository action"
  actual:   2xx CreateRepository success, repository created in emulator state (the check had pre-registered a force-delete teardown precisely for a target that wrongly allows the create).

Corroboration from the same replay: the open issue 20261023T055032Z-ecr-public-getauthorizationtoken-is-not-authorization-gated-stored.md records that an access key for a policy-less IAM user receives HTTP 200 and a usable authorizationToken from GetAuthorizationToken. Taken with this failure, the emulator authorizes no ecr-public request against any identity policy — the authorization step is allow-all.

Bootstrap attribution: the check id's leading token is the composite suite token ecr-public__iam (the check declares SERVICES = ["ecr-public", "iam"]), which is not itself a service. Both constituent services ARE bootstrapped — service-com-amazonaws-ecrpublic-spencerfrontendservice and service-com-amazonaws-iam-awsidentitymanagementv20100508 are both in the bootstrapped set — and both answered in this same replay (61 ecr-public checks passed; every iam: and sts: call in this check succeeded). This is therefore a fidelity divergence in a service that is standing and answering, not a bootstrap gap.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, and replay the ecr-public__iam suite with run_behavioral_suite. Prefer a per-check budget above 60s so the failing beat is reported rather than the run being cut off mid-poll.
2. iam:CreateUser a fresh user; iam:PutUserPolicy an inline policy on it whose single statement is Effect Allow, Resource "*", Action exactly ["ecr-public:DescribeRepositories", "ecr-public:GetAuthorizationToken", "sts:GetServiceBearerToken"]; iam:CreateAccessKey for it.
3. Call sts:GetCallerIdentity signed with that key and confirm Arn is the created user's ARN, not the harness principal's (this already holds today).
4. Signed with that key, against us-east-1, call ecr-public DescribeRepositories and GetAuthorizationToken: both must succeed, and must succeed BECAUSE the policy Allows them.
5. Signed with the SAME key, call ecr-public CreateRepository with a fresh repositoryName. OBSERVED: it returns 2xx and creates the repository. EXPECTED: AccessDeniedException whose message is "User: <the ARN from step 3> is not authorized to perform: ecr-public:CreateRepository on resource: arn:aws:ecr-public::<caller account id>:repository/<the repositoryName sent in this request> because no identity-based policy allows the ecr-public:CreateRepository action".
6. As the administrative/harness principal, call ecr-public DescribeRepositories with repositoryNames=[that name]: it must answer RepositoryNotFoundException, i.e. the denied create left no state behind.
7. iam:PutUserPolicy the same policy name with "ecr-public:CreateRepository" added and nothing else changed, then repeat step 5's identical call: it must now succeed, and the administrative DescribeRepositories must report the repository with registryId equal to the caller's account id and a repositoryUri of the form public.ecr.<domain>/<alias>/<repositoryName>.
