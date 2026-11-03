Issue-Source: trace-sweep-divergence
Issue-Trace: ds-madl-traj-create-microsoft-ad-request-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ds-madl-traj-create-microsoft-ad-request-mistakes ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes ds-madl-traj-directory-lookup-by-id ds-madl-traj-register-certificate-mistakes
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds|CreateMicrosoftAD|4xx->2xx|$.Message
Issue-Key: ds|CreateMicrosoftAD|4xx->2xx|$.RequestId
Issue-Key: ds|DescribeDirectories|4xx->4xx|$.Message
Issue-Key: ds|DescribeDirectories|4xx->4xx|$.message
Issue-Key: ds|RegisterCertificate|4xx->4xx|$.message
Issue-Key: ds|DescribeCertificate|4xx->4xx|$.Message
Issue-Key: ds|DescribeCertificate|4xx->4xx|$.__type
Issue-Key: ds|DeregisterCertificate|4xx->4xx|$.Message
Issue-Key: ds|DeregisterCertificate|4xx->4xx|$.__type
Issue-Related: 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md
Issue-Commit: eea73b787e75fa69d7e1f25d97c4dc355e67b336

# Directory Service certificate and directory lookups: unvalidated ShortName, wrong error class, mis-worded validation and not-found messages

> _Note: this issue was identified against the code base at commit `eea73b787e75fa69d7e1f25d97c4dc355e67b336`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Six net-new wire divergences in the Directory Service (ds) control plane, found by replaying trace set simparity-wire-aws-v1 against the emulator. They share one fault: the refusal the emulator builds for a ds request is not the refusal AWS builds — one modelled pattern constraint is not enforced at all, two refusals are raised as the wrong modelled error class or with re-worded text, one list-constraint body is aggregated differently from AWS, and one echoed pattern carries an anchor AWS does not report. A fixer must close ALL SIX causes before submitting.

Cause 1 of 6 — CreateMicrosoftAD accepts a ShortName that violates the modelled pattern and creates the directory.
AWS: Smithy: CreateMicrosoftAD.ShortName targets DirectoryShortName with @pattern "^[^\\\\/:*?\"<>|.]+[^\\\\/:*?\"<>|]*$"; the operation models InvalidParameterException (client, HTTP 400) whose members are Message and RequestId. A ShortName that fails the rule is refused BEFORE any directory is created, with HTTP 400 and __type "InvalidParameterException". Message template — fixed text, verbatim including the LEADING space and the ' : RequestId: ' separator: " Invalid directory short name. The short name must be between 1 and 15 characters, and can consist of alphanumeric characters and some special characters (! @ # $ % ^ & ( ) - _ ' { } or ~). The following special characters are not allowed: ^ \\ / : * ? \" < > . and |. The name cannot begin with a period. : RequestId: <request id>". The only variable part is the trailing request id, which is also echoed in the response member $.RequestId; its value is per-run and is NOT itself a requirement — the requirement is that $.RequestId is present and equals the id embedded in $.Message.
Emulator: answers HTTP 200 with a created directory; no __type, no $.Message and no $.RequestId are emitted (replay: "status: expected 400, got 200", "$.__type: missing", "$.Message: missing", "$.RequestId: missing"). The cluster covers 8 steps across two fixtures, so every short-name mistake in the corpus is silently accepted.
Why: a client that relies on AWS rejecting a malformed NetBIOS short name gets a Managed Microsoft AD that AWS would never have created, under a name AD itself would reject, and never sees the InvalidParameterException its error handling branches on. Validation tests pass against the emulator and fail against AWS.
Fix: CreateMicrosoftAD's request validation, run before the handler mutates directory state, plus the ds InvalidParameterException serializer that emits Message/RequestId.
Traces: ds-madl-traj-create-microsoft-ad-request-mistakes#task#5 (also task#6, task#7) and ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes#task#7..task#11.

Cause 2 of 6 — DescribeDirectories' unknown-directory message is re-worded.
AWS: Smithy: DescribeDirectories.DirectoryIds targets DirectoryIds, a list whose member carries @pattern "^d-[0-9a-f]{10}$"; the operation models EntityDoesNotExistException (client, 400) with members Message and RequestId. For a well-formed directory id that no directory holds, AWS answers 400 EntityDoesNotExistException. Message template — fixed text: "Invalid directory ID(s): " + <the offending ids, comma-space separated> + ". : RequestId: " + <request id>. Variable parts: the directory id(s) taken from the request and the per-run request id. Note AWS phrases this as an INVALID-ID report naming the ids, unquoted, not as a per-directory "does not exist" sentence.
Emulator: HTTP 400 with the right __type but "Directory: 'd-90000ff0ab' does not exist. : RequestId: ..." — different fixed text entirely, and the id is single-quoted where AWS leaves it bare (replay: "$.Message: value_mismatch").
Why: clients and recorded-response assertions that match the error text to tell a bad-id lookup apart from other EntityDoesNotExistException cases never match, and the message surfaced to a user differs from AWS's.
Fix: the DescribeDirectories handler's unknown-id branch and the ds not-found message builder it calls — emit the "Invalid directory ID(s): <ids>." form with unquoted ids.
Traces: ds-madl-traj-directory-lookup-by-id#task#1 (also task#4).

Cause 3 of 6 — DescribeDirectories reports list-member pattern violations per element instead of aggregated per list member.
AWS: Smithy: DirectoryIds is a list of DirectoryId @pattern "^d-[0-9a-f]{10}$". When entries of ONE list violate the member constraint, AWS reports ONE validation error for the LIST member, not one per element, with HTTP 400 and __type "ValidationException" (framework-level class, not in the operation's modelled error list — recorded behavior). Message template — fixed text: "1 validation error detected: Value '[<offending entries, comma-space separated>]' at '<list member path>' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: <pattern>]"; variable parts: the offending entries and the list path ('directoryIds') from the request, and the pattern from the model. The count prefix therefore stays 1 for a single list, and per-element indices never appear in the path.
Emulator: status and __type match, but it emits "1 validation error detected: Value 'd-90000ff0a' at 'directoryIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$" — a per-element '.N.member' path, the bare value instead of the bracketed list, and no "Member must satisfy constraint: [...]" wrapper.
Why: clients that parse the validation body to map an error back to a request field surface 'directoryIds.1.member', a path AWS never produces, instead of 'directoryIds'; and a request with several bad entries yields a different error count than AWS.
Fix: the shared ds constraint-violation aggregator/serializer that builds the "N validation error(s) detected" body — list-member violations must collapse to one entry keyed by the list's own path. This is the same aggregator named by open issue 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or (its cause 2, on CreateMicrosoftAD.VpcSettings.SubnetIds); that issue carries no replay key for DescribeDirectories, so this step is filed here and must be re-replayed in its own right.
Traces: ds-madl-traj-directory-lookup-by-id#task#2 (also task#3).

Cause 4 of 6 — RegisterCertificate echoes the oCSPUrl pattern with a trailing '$' anchor AWS does not report.
AWS: Smithy: RegisterCertificate.ClientCertAuthSettings targets ClientCertAuthSettings, whose OCSPUrl member carries @length 1..1024 and @pattern "^(https?|ftp|file|ldaps?)://[-a-zA-Z0-9+&@#/%?=~_|!:,.;()]*[-a-zA-Z0-9+&@#/%=~_|()]$". A malformed URL is refused with HTTP 400 and a ValidationException body. Recorded behavior: the pattern AWS QUOTES BACK in the message omits the trailing '$' anchor — fixed text "1 validation error detected: Value '<offending value>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(https?|ftp|file|ldaps?)://[-a-zA-Z0-9+&@#/%?=~_|!:,.;()]*[-a-zA-Z0-9+&@#/%=~_|()]", with the offending value and the member path ('clientCertAuthSettings.oCSPUrl', lower-camel first letter on the member too) taken from the request. Hypothesis: the service's internal constraint text is anchored only at the start, which is why the echoed pattern ends at the character class.
Emulator: identical body except it appends the '$' from the public model: "... [-a-zA-Z0-9+&@#/%=~_|()]$" (replay: "$.message: value_mismatch").
Why: a client or test that compares the returned constraint text against the pattern AWS documents/returns — or that re-uses the echoed pattern to pre-validate input client-side — sees a string AWS never sends, so exact-match assertions on the validation message fail.
Fix: the ds constraint-violation serializer's pattern-rendering step for OCSPUrl — emit the pattern text AWS reports rather than the model's anchored form.
Traces: ds-madl-traj-register-certificate-mistakes#task#4.

Cause 5 of 6 — DescribeCertificate raises DirectoryDoesNotExistException where AWS raises CertificateDoesNotExistException.
AWS: Smithy: DescribeCertificate models BOTH CertificateDoesNotExistException ("The certificate is not present in the system for describe or deregister activities.", client, 400) and DirectoryDoesNotExistException (client, 400); DirectoryId is @required @pattern "^d-[0-9a-f]{10}$" and CertificateId is @required @pattern "^c-[0-9a-f]{10}$". For a describe of a certificate that is not registered, the recorded response is HTTP 400 with __type "CertificateDoesNotExistException"; certificate resolution outranks the directory check, so the certificate class wins. The recording does not assert the message text (shaped, not asserted), so the requirement here is the error CLASS plus a Message/RequestId pair of that class's shape.
Emulator: HTTP 400 with __type "DirectoryDoesNotExistException" and a directory-flavoured message ("Directory: 'd-90000ff0ab' does not exist. : RequestId: ..."), i.e. it refuses on the directory before it ever considers the certificate (replay: "$.__type: value_mismatch (expected 'CertificateDoesNotExistException', got 'DirectoryDoesNotExistException')").
Why: the two classes mean different things — a caller branching on __type reports a missing DIRECTORY and abandons the certificate workflow, where AWS tells it the directory is fine and only the certificate is absent, which is the branch that triggers a re-register.
Fix: DescribeCertificate's handler, in the ordering of its existence checks — resolve the certificate and classify its absence as CertificateDoesNotExistException before falling back to the directory-level refusal.
Traces: ds-madl-traj-register-certificate-mistakes#task#7.

Cause 6 of 6 — DeregisterCertificate makes the same mis-classification.
AWS: Smithy: DeregisterCertificate models CertificateDoesNotExistException, CertificateInUseException, DirectoryDoesNotExistException and DirectoryUnavailableException (all client, 400), with the same @required/@pattern DirectoryId and CertificateId. Deregistering a certificate that is not registered yields HTTP 400 __type "CertificateDoesNotExistException"; the message is shaped but not asserted in the recording.
Emulator: HTTP 400 __type "DirectoryDoesNotExistException" with the directory not-found message, same as cause 5 (replay: "$.__type: value_mismatch (expected 'CertificateDoesNotExistException', got 'DirectoryDoesNotExistException')").
Why: same as cause 5; because the defect reaches both certificate read paths, fixing only DescribeCertificate leaves DeregisterCertificate diverging, so an idempotent "remove if present" client still mis-reports.
Fix: the same check-ordering in DeregisterCertificate's handler — single-home the certificate-resolution step shared with DescribeCertificate and assert both.
Traces: ds-madl-traj-register-certificate-mistakes#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 18 fixtures, 101 steps: 66 passed, 35 mismatched, 0 errored, 0 blocked; score 65.3%; 12 divergence clusters, all service "ds"). Six clusters are already owned by 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md and are not repeated here; the six below are net-new.

1) ds-madl-traj-create-microsoft-ad-request-mistakes#task#5 — CreateMicrosoftAD, expected 400 / actual 200 (cluster of 8 steps: same fixture task#6, task#7; ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes task#7..task#11).
   expected: status 400; $.__type "InvalidParameterException"; $.Message " Invalid directory short name. The short name must be between 1 and 15 characters, and can consist of alphanumeric characters and some special characters (! @ # $ % ^ & ( ) - _ ' { } or ~). The following special characters are not allowed: ^ \\ / : * ? \" < > . and |. The name cannot begin with a period. : RequestId: <uuid>"; $.RequestId <uuid>.
   actual: status 200, $.__type missing, $.Message missing, $.RequestId missing.
   Fixed text = everything up to " : RequestId: " (including the leading space); variable = the request id (per-run, not a requirement beyond being echoed consistently in $.RequestId).

2) ds-madl-traj-directory-lookup-by-id#task#1 — DescribeDirectories, 400 vs 400, $.Message value_mismatch (cluster also covers task#4).
   expected: "Invalid directory ID(s): d-90000ff0ab. : RequestId: <uuid>"
   actual:   "Directory: 'd-90000ff0ab' does not exist. : RequestId: <uuid>"
   Fixed text diverges wholesale ("Invalid directory ID(s): <ids>." vs "Directory: '<id>' does not exist."); the directory id is a request value and the RequestId is per-run.

3) ds-madl-traj-directory-lookup-by-id#task#2 — DescribeDirectories, 400 vs 400, $.message value_mismatch (cluster also covers task#3).
   expected: "1 validation error detected: Value '[d-90000ff0a]' at 'directoryIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$]"
   actual:   "1 validation error detected: Value 'd-90000ff0a' at 'directoryIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$"
   Diverging fixed text: the bracketed value list, the list path without '.N.member', and the "Member must satisfy constraint: [...]" wrapper.

4) ds-madl-traj-register-certificate-mistakes#task#4 — RegisterCertificate, 400 vs 400, $.message value_mismatch.
   expected: "1 validation error detected: Value 'not-a-valid-url' at 'clientCertAuthSettings.oCSPUrl' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(https?|ftp|file|ldaps?)://[-a-zA-Z0-9+&@#/%?=~_|!:,.;()]*[-a-zA-Z0-9+&@#/%=~_|()]"
   actual:   identical but with a trailing "$" appended to the pattern.
   Diverging fixed text: the trailing '$' anchor; the offending value and member path are request-derived.

5) ds-madl-traj-register-certificate-mistakes#task#7 — DescribeCertificate, 400 vs 400: "$.__type: value_mismatch (expected 'CertificateDoesNotExistException', got 'DirectoryDoesNotExistException')"; $.Message expected shaped-not-asserted, actual "Directory: 'd-90000ff0ab' does not exist. : RequestId: ...".

6) ds-madl-traj-register-certificate-mistakes#task#9 — DeregisterCertificate, 400 vs 400: same __type mismatch and same directory-flavoured message as (5).

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): CreateMicrosoftAD.ShortName -> DirectoryShortName @pattern ^[^\\\\/:*?\"<>|.]+[^\\\\/:*?\"<>|]*$, errors include InvalidParameterException{Message,RequestId} (client, 400); DescribeDirectories.DirectoryIds -> list of DirectoryId @pattern ^d-[0-9a-f]{10}$, errors include EntityDoesNotExistException{Message,RequestId}; ClientCertAuthSettings.OCSPUrl @length 1..1024 @pattern ^(https?|ftp|file|ldaps?)://[-a-zA-Z0-9+&@#/%?=~_|!:,.;()]*[-a-zA-Z0-9+&@#/%=~_|()]$; DescribeCertificate and DeregisterCertificate each model CertificateDoesNotExistException AND DirectoryDoesNotExistException (both client, 400) with @required DirectoryId/CertificateId. The echoed-pattern text in cause 4 and the check ordering in causes 5/6 are grounded in the recordings, not the model.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (all six causes are ds control-plane calls; no live account needed).
2. Cause 1: fixture ds-madl-traj-create-microsoft-ad-request-mistakes, step task#5 — CreateMicrosoftAD with a ShortName that violates DirectoryShortName's pattern; observe HTTP 200 with a created DirectoryId instead of 400 InvalidParameterException carrying the " Invalid directory short name. ..." Message and a RequestId. Repeat on task#6/task#7 and on ds-madl-traj-create-microsoft-ad-subnet-pick-mistakes task#7..task#11.
3. Cause 2: fixture ds-madl-traj-directory-lookup-by-id, step task#1 — DescribeDirectories with DirectoryIds=[d-90000ff0ab], a well-formed id no directory holds; observe $.Message "Directory: 'd-90000ff0ab' does not exist." instead of "Invalid directory ID(s): d-90000ff0ab.".
4. Cause 3: same fixture, step task#2 — DescribeDirectories with DirectoryIds=[d-90000ff0a] (one char short); observe the per-element 'directoryIds.1.member' path instead of AWS's aggregated 'directoryIds' error with the bracketed value list.
5. Cause 4: fixture ds-madl-traj-register-certificate-mistakes, step task#4 — RegisterCertificate with ClientCertAuthSettings.OCSPUrl="not-a-valid-url"; diff the returned $.message against the expected text and observe the extra trailing '$' on the echoed pattern.
6. Cause 5: same fixture, step task#7 — DescribeCertificate for a certificate that is not registered; observe __type DirectoryDoesNotExistException instead of CertificateDoesNotExistException.
7. Cause 6: same fixture, step task#9 — DeregisterCertificate for the same unregistered certificate; observe the same wrong __type.
