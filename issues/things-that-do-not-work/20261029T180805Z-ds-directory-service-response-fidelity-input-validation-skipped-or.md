Issue-Source: trace-sweep-divergence
Issue-Trace: directory-service-create-microsoft-ad-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: directory-service-create-microsoft-ad-validation-errors ds-account-directory-inventory-reads ds-create-computer-and-reset-password-error-paths-fake-directory ds-trust-not-found-errors
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|CreateMicrosoftAD|4xx->2xx|$.__type
Issue-Key: ds|CreateMicrosoftAD|4xx->2xx|$.message
Issue-Key: ds|CreateMicrosoftAD|4xx->2xx|status
Issue-Key: ds|CreateMicrosoftAD|4xx->4xx|$.message
Issue-Key: ds|GetDirectoryLimits|2xx->2xx|$.DirectoryLimits.IdentityPoolDirectoriesLimitReached
Issue-Key: ds|CreateComputer|4xx->4xx|$.__type
Issue-Key: ds|VerifyTrust|4xx->4xx|$.Message
Issue-Key: ds|DeleteTrust|4xx->4xx|$.Message
Issue-Commit: ab6490681e1ab26d5567ff83a1d290924eb5dab0

# Directory Service response fidelity: input validation skipped or mis-aggregated, wrong error class, omitted limit member and mis-worded not-found messages (6 causes)

> _Note: this issue was identified against the code base at commit `ab6490681e1ab26d5567ff83a1d290924eb5dab0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Six net-new wire divergences in the Directory Service (ds) control plane, all found by replaying the recorded corpus against the emulator. They share one fault: the response the emulator builds for a ds call is not the response AWS builds — constraint validation on the request is either not run or aggregated differently, one modelled success member is omitted, one error is raised as the wrong modelled class, and the not-found message text is re-worded. A fixer must close ALL SIX before submitting.

Cause 1 of 6 — CreateMicrosoftAD accepts a Password that violates the modelled pattern.
AWS: Smithy: CreateMicrosoftAD.Password is @required, @sensitive and carries @pattern "^(?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^". A password failing it is refused BEFORE any directory is created, with HTTP 400 and (recorded behavior; this framework-level class is not in the operation's modelled error list) __type "ValidationException". Message template — fixed text: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; variable parts: the member path taken from the request ('password') and the pattern from the model. Because Password is @sensitive the rejected value is NOT echoed — the clause is "Value at 'password'", not "Value '<secret>' at 'password'".
Emulator: answers HTTP 200 with a created directory; no __type and no message are emitted at all (replay: "status: expected 400, got 200", "$.__type: missing", "$.message: missing").
Why: a client that relies on AWS rejecting a weak admin password gets a Managed Microsoft AD that AWS would never have created, and never sees the ValidationException its error handling is written against; password-policy tests pass against the emulator and fail against AWS.
Fix: CreateMicrosoftAD's request validation, run before the handler mutates state, plus the shared constraint-violation serializer that words "N validation error(s) detected: ..." bodies.
Traces: directory-service-create-microsoft-ad-validation-errors#task#1.

Cause 2 of 6 — list-member constraint violations are reported per element instead of aggregated per list member.
AWS: Smithy: CreateMicrosoftAD.VpcSettings is @required; DirectoryVpcSettings.SubnetIds is @required and its members carry @pattern "^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$". When several entries of ONE list violate the member constraint, AWS reports ONE validation error for the list member, not one per element. Message template — fixed text: "1 validation error detected: Value '[<offending entries, comma-space separated>]' at '<list member path>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: <pattern>, Member must not be null]"; variable parts: the offending entries and the list path ('vpcSettings.subnetIds') from the request and the pattern from the model. The error COUNT in the prefix therefore stays 1 for a single list, and element indices never appear in the path.
Emulator: status and __type match, but it emits one error per element: "2 validation errors detected: Value 'bad-subnet-1' at 'vpcSettings.subnetIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ...; Value 'bad-subnet-2' at 'vpcSettings.subnetIds.2.member' failed to satisfy constraint: ..." — wrong count prefix, per-element '.N.member' paths, no bracketed value list and no "Member must satisfy constraint: [... , Member must not be null]" wrapper.
Why: clients and test suites that parse or assert the validation message (count prefix, the member path they must surface to a user) read a different number of errors and a path AWS never produces, so field-level error mapping points at 'vpcSettings.subnetIds.1.member' instead of 'vpcSettings.subnetIds'.
Fix: the shared constraint-violation aggregator/serializer that builds the "N validation errors detected" body — list-member violations must collapse to one entry keyed by the list's path.
Traces: directory-service-create-microsoft-ad-validation-errors#task#3.

Cause 3 of 6 — GetDirectoryLimits omits DirectoryLimits.IdentityPoolDirectoriesLimitReached.
AWS: recorded behavior only — the on-disk model's DirectoryLimits structure declares only the CloudOnly*/CloudOnlyMicrosoftAD*/Connected* members, but the recorded 200 response carries DirectoryLimits.IdentityPoolDirectoriesLimitReached as a boolean, false for an account that has not reached the identity-pool directory limit. The sibling limit/count members the recording asserts already match.
Emulator: the key is absent from the DirectoryLimits object (replay: "$.DirectoryLimits.IdentityPoolDirectoriesLimitReached: missing (expected False, got None)").
Why: a client that reads every limit flag off the response to decide whether another directory can be created sees an absent (null) flag where AWS always sends an explicit boolean, so the "limit reached?" check is undefined rather than false.
Fix: the GetDirectoryLimits response projection for the account's directory-limit state — emit the member with its false default alongside the other *LimitReached flags.
Traces: ds-account-directory-inventory-reads#task#2.

Cause 4 of 6 — CreateComputer against a directory that is not serving raises the wrong modelled error class.
AWS: Smithy: CreateComputer models both DirectoryUnavailableException ("The specified directory is unavailable.") and EntityDoesNotExistException ("The specified entity could not be found."), each client-kind, HTTP 400. For a well-formed DirectoryId (^d-[0-9a-f]{10}$) whose directory cannot service the join, the recorded response is HTTP 400 with __type "DirectoryUnavailableException".
Emulator: HTTP 400 with __type "EntityDoesNotExistException" (replay: "$.__type: value_mismatch (expected 'DirectoryUnavailableException', got 'EntityDoesNotExistException')").
Why: the two classes mean different things to a caller — unavailable is retryable/wait-for-Active, does-not-exist is terminal. A client branching on __type retries nothing and reports a missing directory where AWS reports a temporarily unusable one.
Fix: CreateComputer's handler, in the branch that resolves the DirectoryId before the computer join — the refusal for a well-formed but non-serving directory must be classified DirectoryUnavailableException.
Traces: ds-create-computer-and-reset-password-error-paths-fake-directory#task#1.

Cause 5 of 6 — VerifyTrust's trust-not-found message is re-worded (quotes added, sentence terminated).
AWS: Smithy: VerifyTrust.TrustId is @required with @pattern "^t-[0-9a-f]{10}$" and the operation models EntityDoesNotExistException (client, 400) whose members are Message and RequestId. Message template — fixed text: "Trust: " + <trust id> + " does not exist : RequestId: " + <request id>; the trust id is taken verbatim from the request UNQUOTED, there is no full stop after "exist", and exactly one space precedes the " : RequestId: " separator. The request id itself is per-run and is NOT part of the requirement.
Emulator: wraps the id in single quotes and terminates the clause: "Trust: 't-1234567890' does not exist. : RequestId: ..." where AWS sends "Trust: t-1234567890 does not exist : RequestId: ..." — the quotes and the '.' are the diverging fixed text.
Why: clients (and recorded-response assertions) that match the error text to distinguish a missing trust from other EntityDoesNotExistException cases fail to match, and user-facing messages differ from AWS's.
Fix: the ds error-message builder for the trust not-found refusal (shared by the trust operations) — do not quote the identifier and do not append a full stop.
Traces: ds-trust-not-found-errors#task#2.

Cause 6 of 6 — DeleteTrust's trust-not-found message is re-worded the same way.
AWS: Smithy: DeleteTrust.TrustId is @required with @pattern "^t-[0-9a-f]{10}$"; the operation models EntityDoesNotExistException (client, 400). Same message template as cause 5 — fixed text "Trust: <trust id> does not exist : RequestId: <request id>", identifier unquoted, no terminating full stop; the trust id comes from the request and the request id is per-run.
Emulator: "Trust: 't-1234567890' does not exist. : RequestId: ..." — quoted id and trailing '.' again.
Why: same as cause 5; the defect reaches every trust operation that reports a missing trust, so fixing only VerifyTrust leaves DeleteTrust diverging.
Fix: same ds trust not-found message builder; assert both VerifyTrust and DeleteTrust once it is single-homed.
Traces: ds-trust-not-found-errors#task#4.

Related open issues that do NOT cover these (no replay key, so none owns a step above): 20261022T175927Z-ds-directory-creation-skips-password-and-vpc-validation and 20261024T015148Z-ds-directory-service-a-new-microsoft-ad-is-instantly both discuss CreateMicrosoftAD validation behaviorally; 20261024T081601Z-ds-directory-service-control-plane-7-behavioral-divergences-error discusses ds error envelopes. Causes 1 and 2 are filed here because they are wire-replay divergences bound to the fixture steps named above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 54 steps, 35 passed, 19 mismatched, 7 divergence clusters — all service "ds"):

1) directory-service-create-microsoft-ad-validation-errors#task#1 — CreateMicrosoftAD, expected 400 / actual 200.
   expected: status 400, $.__type "ValidationException", $.message "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^.*"
   actual: status 200, $.__type missing, $.message missing.

2) directory-service-create-microsoft-ad-validation-errors#task#3 — CreateMicrosoftAD, 400 vs 400, $.message value_mismatch.
   expected: "1 validation error detected: Value '[bad-subnet-1, bad-subnet-2]' at 'vpcSettings.subnetIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: ^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$, Member must not be null]"
   actual: "2 validation errors detected: Value 'bad-subnet-1' at 'vpcSettings.subnetIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$; Value 'bad-subnet-2' at 'vpcSettings.subnetIds.2.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(subnet-[0-9a-f]{8}|subnet-[0-9a-f]{17})$"

3) ds-account-directory-inventory-reads#task#2 — GetDirectoryLimits, 200 vs 200: "$.DirectoryLimits.IdentityPoolDirectoriesLimitReached: missing (expected False, got None)".

4) ds-create-computer-and-reset-password-error-paths-fake-directory#task#1 — CreateComputer, 400 vs 400: "$.__type: value_mismatch (expected 'DirectoryUnavailableException', got 'EntityDoesNotExistException')".

5) ds-trust-not-found-errors#task#2 — VerifyTrust, 400 vs 400: expected $.Message "Trust: t-1234567890 does not exist : RequestId: <uuid>", actual "Trust: 't-1234567890' does not exist. : RequestId: <uuid>". Diverging fixed text: the single quotes around the trust id and the full stop after "exist"; the trust id is a request value and the RequestId is per-run (not asserted as a requirement).

6) ds-trust-not-found-errors#task#4 — DeleteTrust, 400 vs 400: same expected/actual wording divergence as (5).

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416): CreateMicrosoftAD.Password @required @sensitive @pattern as quoted; DirectoryVpcSettings.SubnetIds @required with subnet-id member pattern; CreateComputer models DirectoryUnavailableException and EntityDoesNotExistException (both client, 400); VerifyTrust/DeleteTrust TrustId @pattern ^t-[0-9a-f]{10}$ with EntityDoesNotExistException{Message,RequestId}; DirectoryLimits declares no IdentityPoolDirectories* member, so cause 3 is grounded in the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: fixture directory-service-create-microsoft-ad-validation-errors, step task#1 — CreateMicrosoftAD with a Password that violates the modelled pattern; observe 200 + a created DirectoryId instead of 400 ValidationException.
3. Cause 2: same fixture, step task#3 — CreateMicrosoftAD with VpcSettings.SubnetIds = [bad-subnet-1, bad-subnet-2]; observe a "2 validation errors detected" body with per-element paths instead of AWS's single aggregated 'vpcSettings.subnetIds' error.
4. Cause 3: fixture ds-account-directory-inventory-reads, step task#2 — GetDirectoryLimits; observe DirectoryLimits with no IdentityPoolDirectoriesLimitReached key.
5. Cause 4: fixture ds-create-computer-and-reset-password-error-paths-fake-directory, step task#1 — CreateComputer against a well-formed DirectoryId that is not serving; observe __type EntityDoesNotExistException instead of DirectoryUnavailableException.
6. Causes 5 and 6: fixture ds-trust-not-found-errors, steps task#2 (VerifyTrust) and task#4 (DeleteTrust) with a TrustId no trust holds; observe the Message quoting the trust id and ending "does not exist." instead of "Trust: <id> does not exist : RequestId: <id>".
