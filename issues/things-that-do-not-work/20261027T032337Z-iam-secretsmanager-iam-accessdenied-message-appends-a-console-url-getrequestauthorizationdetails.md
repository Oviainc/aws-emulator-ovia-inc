Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol
Issue-Key: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol|run#16|AssertionError|assert iam_msg == want, f"IAM denial message: expected {want!r}, got {iam_msg!r}"
Issue-Commit: f9dabc8a716cd35aa702facadefbd80952bd273f

# IAM AccessDenied message appends a console-URL / GetRequestAuthorizationDetails suffix that AWS does not emit, so the denial text no longer matches AWS's documented wording

> _Note: this issue was identified against the code base at commit `f9dabc8a716cd35aa702facadefbd80952bd273f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUsers

## Description
One root cause. Cause count: 1.

Related but NOT covering: open issue 20261024T190718Z-iam-secretsmanager-iam-authorization-is-not-enforced-for-a-user.md carries the replay key for the EARLIER failure point of this same check (`_error_code_and_status#12` / 'expected the call to be denied, but it SUCCEEDED'). The emulator now DOES refuse the unauthorized call with AccessDenied, and the check advances past that assertion; it now fails on a different, net-new defect in the denial MESSAGE, which that issue's key does not cover and which the suite did not mark OWNED.

=== Cause 1 of 1: the IAM implicit-deny AccessDenied message carries an extra trailing sentence AWS does not produce ===
AWS: IAM speaks awsQuery (Smithy: service AWSIdentityManagementV20100508, version 2010-05-08, protocols [awsQuery]; the implicit-deny AccessDenied is a protocol-level/common error, so its message text is not modeled — recorded behavior only). For an action refused because the signing principal has no identity-based policy allowing it, the recorded AWS message is exactly one sentence and nothing more:
  `User: <principal-arn> is not authorized to perform: <service>:<Action> on resource: <resource-arn> because no identity-based policy allows the <service>:<Action> action`
Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts taken from the request and from the emulator's state: `<principal-arn>` (the ARN of the principal whose access key signed the request), `<service>:<Action>` (the IAM action name of the operation being refused, e.g. `iam:ListUsers`), and `<resource-arn>` (the resource the action was evaluated against). The sentence ENDS at `action` — there is no trailing period and no further sentence.
Emulator: the emulator returns the correct code (`AccessDenied`) and the correct leading sentence, then appends a second sentence that AWS never emits:
  `... because no identity-based policy allows the iam:ListUsers action. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>`
The appended text is invented prose (it names a console URL and a `GetRequestAuthorizationDetails` API that is not part of the IAM 2010-05-08 surface) plus a per-run generated authorization id. The divergence is NOT the per-run id — it is the whole extra sentence, which is present on every such refusal.
Why: a client that reads or asserts on AWS's documented denial text gets a different string. Least-privilege tooling, policy-troubleshooting UIs and negative IAM tests that parse or compare `because no identity-based policy allows the <action> action` as the message tail fail against the emulator, and log scrapers keyed on the message see an extra clause pointing at a console URL and an API that does not exist. It also makes the message non-deterministic across runs for no AWS-observable reason.
Fix: the shared request-authorization denial path that formats IAM's implicit-deny AccessDenied message — it must emit only the single documented sentence, filling the principal ARN, the IAM action and the resource ARN from the request and state, and must not append the console-URL / authorization-id clause.
Traces: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol (assertion at check line 111/177, `assert iam_msg == want`).

## Evidence
Check `iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol`, verdict mismatch:

AssertionError: IAM denial message: expected 'User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user/ because no identity-based policy allows the iam:ListUsers action', got 'User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: iam:ListUsers on resource: arn:aws:iam::<account>:user/ because no identity-based policy allows the iam:ListUsers action. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<auth-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <auth-id>'

The error code (`AccessDenied`) matched; only the message diverged, by the appended second sentence. The check's positive controls (iam ListUsers and secretsmanager ListSecrets with the account's own credentials) both succeeded first, so this is the principal-specific denial path and not an account-wide effect.

## Steps to Reproduce
1. iam CreateUser(UserName=<u>) — a user with no attached or inline policy.
2. iam CreateAccessKey(UserName=<u>).
3. Build an iam client signing with that access key alone (no session token).
4. Call iam ListUsers(MaxItems=1) with it.
5. Observe HTTP 403 / Error.Code `AccessDenied` (correct) but Error.Message with a trailing ' Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>' sentence appended after '... because no identity-based policy allows the iam:ListUsers action'. AWS's message ends at 'action'.
