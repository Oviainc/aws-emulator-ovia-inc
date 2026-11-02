Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.describe-directories-id-outside-the-d-hex10-pattern-rejected-validation ds.create-microsoft-ad-simultaneous-modeled-pattern-violations-ordered-in-one-validation-exception ds.create-microsoft-ad-single-label-domain-name-rejected-validation ds.register-certificate-unknown-directory-refused-before-body-validation ds.delete-directory-unknown-id-rejected-entity-does-not-exist ds.e2e-trust-create-verify-delete-flow
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.describe-directories-id-outside-the-d-hex10-pattern-rejected-validation|run#6|AssertionError|assert msg == expected_msg, f"unexpected ValidationException message: {msg!r}"
Issue-Key: ds.create-microsoft-ad-simultaneous-modeled-pattern-violations-ordered-in-one-validation-exception|run#12|AssertionError|assert msg == f"2 validation errors detected: {_PASSWORD_VIOLATION}; {_name_violation(label)}", (
Issue-Key: ds.create-microsoft-ad-single-label-domain-name-rejected-validation|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ds.register-certificate-unknown-directory-refused-before-body-validation|run#4|AssertionError|assert code == "DirectoryDoesNotExistException", (
Issue-Key: ds.delete-directory-unknown-id-rejected-entity-does-not-exist|run#8|AssertionError|assert control == "EntityDoesNotExistException", (
Issue-Key: ds.e2e-trust-create-verify-delete-flow|run#9|KeyError|dns_a = desc_a["DnsIpAddrs"]
Issue-Commit: 855e46d3b98757a511fcacb47a7c9e35083e2051

# ds: six net-new validation, ordering and representation root causes on CreateMicrosoftAD, DescribeDirectories, RegisterCertificate and ResetUserPassword

> _Note: this issue was identified against the code base at commit `855e46d3b98757a511fcacb47a7c9e35083e2051`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMicrosoftAD

## Description
Six net-new behavioral root causes in Directory Service (ds), all outside the message-wording family: constraint violations are aggregated and ordered differently from AWS, one request AWS refuses is accepted, one refusal is decided in the wrong order, one operation is still a 501 stub, and one success member is omitted. A fixer must close ALL SIX before submitting.

Cause 1 of 6 — a list-member constraint violation is reported per element instead of aggregated for the list member.
AWS: Smithy: DescribeDirectories.DirectoryIds is a list whose members match @pattern ^d-[0-9a-f]{10}$; the framework-level refusal is HTTP 400 ValidationException (recorded behavior: this class is not in the operation's modeled error list). Message template — fixed text: "1 validation error detected: Value '[" <offending entries, comma-space separated> "]' at '" <list member path> "' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: " <pattern> "]"; variable parts: the offending entries taken from the request, the list path ('directoryIds') and the pattern from the model. One violating list yields ONE error, the bracketed value list, and a path with no element index.
Emulator: right code and status, but "1 validation error detected: Value 'dir-not-an-id' at 'directoryIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$" — an element path '.1.member', no bracketed value list and no "Member must satisfy constraint: [...]" wrapper.
Why: a client mapping the validation error back to a request field surfaces 'directoryIds.1.member', a path AWS never produces, and loses the aggregation AWS guarantees (so its error count and field mapping are both wrong).
Fix: the shared ds constraint-violation aggregator/serializer that renders "N validation error(s) detected: ..." bodies — list-member violations must collapse to one entry keyed by the list's path.
Traces: ds.describe-directories-id-outside-the-d-hex10-pattern-rejected-validation. (The already-filed WIRE issue 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md records the same aggregator defect for CreateMicrosoftAD's vpcSettings.subnetIds; it carries no behavioral check id and does not own this check.)

Cause 2 of 6 — simultaneous modeled pattern violations are not reported together, and the @sensitive Password violation is dropped.
AWS: Smithy: CreateMicrosoftAD.Password is @required, @sensitive, @pattern "(?=^.{8,64}$)((?=.*\\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[^A-Za-z0-9\\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\\s]))^.*"; Name is @required with @pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$. When one request violates both, AWS reports BOTH in a single 400 ValidationException, in input-member order (password before name). Message template — fixed text: "2 validation errors detected: " then the password clause "Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: <password pattern>" then "; " then the name clause "Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: <name pattern>"; variable parts: the name value from the request and the patterns from the model. Because Password is @sensitive its clause elides the value ("Value at 'password'", never "Value '<secret>' at 'password'").
Emulator: answers "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$" — the Password violation is never reported at all, so the count prefix is 1 and the password clause is missing.
Why: a caller fixing the reported problems round-trips twice (or, if it only ever sends a weak password with a valid name, is never told the password is the problem); password-policy handling written against AWS's two-error body breaks.
Fix: CreateMicrosoftAD's request validation (it must evaluate Password's @pattern) plus the shared constraint-violation collector that emits every violation in member order.
Traces: ds.create-microsoft-ad-simultaneous-modeled-pattern-violations-ordered-in-one-validation-exception.

Cause 3 of 6 — CreateMicrosoftAD accepts a single-label (dot-less) directory name AWS refuses.
AWS: recorded behavior for the name member's constraint check — a Name consisting of one label with no dot is refused BEFORE any VPC is consulted, with HTTP 400 ValidationException. Message template — fixed text: "1 validation error detected: Value '" <name> "' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$"; variable part: the name from the request. (Positive control recorded alongside: the same request with a qualified name passes this check and fails later on the fabricated VPC with ClientException, proving the name check is decided first.)
Emulator: the call SUCCEEDS (HTTP 200, a DirectoryId is minted) — no refusal at all.
Why: a caller creating a directory from an unqualified host name gets a Managed Microsoft AD AWS would never create, and its negative test for this input passes against the emulator and fails against AWS.
Fix: CreateMicrosoftAD's Name validation, run before the VPC is resolved.
Traces: ds.create-microsoft-ad-single-label-domain-name-rejected-validation.

Cause 4 of 6 — RegisterCertificate parses the certificate body before resolving the directory, so the wrong refusal wins.
AWS: Smithy: RegisterCertificate models BOTH DirectoryDoesNotExistException and InvalidCertificateException (client, HTTP 400); DirectoryId @pattern ^d-[0-9a-f]{10}$, CertificateData @required @length 1..8192. When a request carries an unknown DirectoryId AND a non-PEM body, the directory lookup runs FIRST, so the response is DirectoryDoesNotExistException. Message template — fixed text: "Directory: '" <directory id> "' does not exist." then " : RequestId: " <request id>; variable part: the directory id from the request. (Positive control: ListCertificates on the same id answers the same code and message.)
Emulator: answers InvalidCertificateException "CertificateData could not be read as a PEM-encoded X.509 certificate : RequestId: ..." — body validation decided before the directory lookup.
Why: a caller retrying against a directory it has not created yet is told its certificate is bad rather than that the directory is missing, so it fixes the wrong thing; the two codes drive different recovery paths.
Fix: RegisterCertificate's handler — resolve DirectoryId against state before parsing CertificateData.
Traces: ds.register-certificate-unknown-directory-refused-before-body-validation.

Cause 5 of 6 — ResetUserPassword is an unimplemented scaffold stub, so an unknown directory answers 501 NotImplemented.
AWS: Smithy: ResetUserPassword takes DirectoryId (@pattern ^d-[0-9a-f]{10}$), UserName and NewPassword, and models EntityDoesNotExistException (client, HTTP 400) — "The specified entity could not be found." For a well-formed DirectoryId no directory holds, the response is HTTP 400 EntityDoesNotExistException with the ds directory-not-found message (fixed text "Directory: '" <directory id> "' does not exist.", the id from the request).
Emulator: HTTP 501 with code NotImplemented; services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/reset_user_password.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and returns crate::scaffolded::Scaffolded::not_implemented("ResetUserPassword").
Why: callers cannot tell a missing directory from an unusable emulator, and 501 is not in the operation's modeled error set at all, so no SDK error branch handles it. It also breaks the cross-API invariant the check exercises: DeleteDirectory and ResetUserPassword must agree that a directory is missing.
Fix: implement ResetUserPassword in src/ops/reset_user_password.rs (at minimum the DirectoryId lookup and its EntityDoesNotExistException refusal) and delete the SCAFFOLDED marker.
Traces: ds.delete-directory-unknown-id-rejected-entity-does-not-exist. (Open issue 20261029T200700Z-ds-7-net-new-root-causes-connectdirectory-describesettings.md lists ResetUserPassword among 501 stubs; it carries no behavioral check id and does not own this check.)

Cause 6 of 6 — DescribeDirectories omits DnsIpAddrs for an Active Managed Microsoft AD.
AWS: Smithy: DirectoryDescription carries DnsIpAddrs (the list of IP addresses of the directory's DNS servers); for an Active Managed Microsoft AD the member is always present, one address per subnet the directory was placed in. Variable parts: the addresses themselves come from the directory's provisioned network and are never fixed values.
Emulator: the key is absent from the DirectoryDescription, so reading it raises KeyError: 'DnsIpAddrs' — the directory reports no DNS servers at all.
Why: every caller that configures a conditional forwarder, a trust or on-premises DNS reads DnsIpAddrs off the describe; an absent member makes the Active directory unusable for the flows it exists for (here the whole create-verify-delete trust flow cannot run).
Fix: the DescribeDirectories response projection for a Microsoft AD, fed from the directory's provisioned controller addresses in state.
Traces: ds.e2e-trust-create-verify-delete-flow. (Open issue 20261024T081028Z-ds-directory-service-a-microsoft-ad-never-provisions-its.md describes the same missing network provisioning for other checks; it does not carry this check id.)

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout (ds: 30/155 passed). RequestIds are per-run and are not requirements.

1) ds.describe-directories-id-outside-the-d-hex10-pattern-rejected-validation — DescribeDirectories(DirectoryIds=['dir-not-an-id']), code ValidationException matches.
   expected: "1 validation error detected: Value '[dir-not-an-id]' at 'directoryIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$]"
   actual:   "1 validation error detected: Value 'dir-not-an-id' at 'directoryIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^d-[0-9a-f]{10}$"

2) ds.create-microsoft-ad-simultaneous-modeled-pattern-violations-ordered-in-one-validation-exception — CreateMicrosoftAD with a Password violating its @pattern AND a Name violating its @pattern.
   expected: "2 validation errors detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: <password pattern>; Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$"
   actual:   "1 validation error detected: Value 'sim2realpw09a806087ff1' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$"

3) ds.create-microsoft-ad-single-label-domain-name-rejected-validation — CreateMicrosoftAD(Name=<single label, no dot>, Edition=Standard, VpcSettings=<fabricated ids>).
   expected: HTTP 400 ValidationException "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$"
   actual:   HTTP 200, a directory is created — harness reports "the call succeeded; real AWS rejects it, so there is no error code".

4) ds.register-certificate-unknown-directory-refused-before-body-validation — RegisterCertificate(DirectoryId=<unknown well-formed id>, CertificateData="not-a-pem-...", Type=ClientLDAPS).
   expected: DirectoryDoesNotExistException "Directory: '<id>' does not exist. : RequestId: <rid>"
   actual:   InvalidCertificateException "CertificateData could not be read as a PEM-encoded X.509 certificate : RequestId: <rid>"
   (The ListCertificates control on the same id is what AWS answers identically.)

5) ds.delete-directory-unknown-id-rejected-entity-does-not-exist — ResetUserPassword(DirectoryId=<unknown well-formed id>).
   expected: EntityDoesNotExistException (HTTP 400)
   actual:   'NotImplemented' (HTTP 501) — "control: ResetUserPassword on the same id should be refused EntityDoesNotExistException, got 'NotImplemented'".
   Code: src/ops/reset_user_password.rs lines 4-6 ("SCAFFOLDED, NOT IMPLEMENTED") and line 174 (Scaffolded::not_implemented("ResetUserPassword")).

6) ds.e2e-trust-create-verify-delete-flow — DescribeDirectories on an Active Managed Microsoft AD.
   expected: DirectoryDescription.DnsIpAddrs present (one address per placement subnet)
   actual:   KeyError: 'DnsIpAddrs' — the member is absent from the response.

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): DescribeDirectories.DirectoryIds list with ^d-[0-9a-f]{10}$ members, models InvalidParameterException/EntityDoesNotExistException; CreateMicrosoftAD.Password @required @sensitive @pattern as quoted and Name @required @pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+$; RegisterCertificate models DirectoryDoesNotExistException and InvalidCertificateException (both client, 400) with CertificateData @required @length 1..8192; ResetUserPassword models EntityDoesNotExistException (client, 400).

## Steps to Reproduce
1. Clone this checkout and replay suite simparity-behavioral-aws-v1 against the booted emulator.
2. Cause 1: DescribeDirectories(DirectoryIds=['dir-not-an-id']) — observe the per-element 'directoryIds.1.member' path instead of the aggregated 'directoryIds' error.
3. Cause 2: CreateMicrosoftAD with BOTH a pattern-violating Password and a pattern-violating Name — observe "1 validation error detected" naming only 'name'.
4. Cause 3: CreateMicrosoftAD(Name=<one label, no dot>, Password=<valid>, Edition=Standard, VpcSettings=<any>) — observe HTTP 200 and a minted DirectoryId instead of a 400 ValidationException on 'name'.
5. Cause 4: RegisterCertificate(DirectoryId=<well-formed id no directory holds>, CertificateData='not-a-pem-'*10, Type=ClientLDAPS) — observe InvalidCertificateException instead of DirectoryDoesNotExistException; compare with ListCertificates(DirectoryId=<same id>).
6. Cause 5: ResetUserPassword(DirectoryId=<well-formed id no directory holds>, UserName=<any>, NewPassword=<any>) — observe HTTP 501 NotImplemented.
7. Cause 6: CreateMicrosoftAD, wait for Stage Active, then DescribeDirectories(DirectoryIds=[<id>]) — observe no DnsIpAddrs key in the DirectoryDescription.
