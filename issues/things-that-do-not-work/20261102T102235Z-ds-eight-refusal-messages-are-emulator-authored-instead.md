Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.accept-shared-directory-already-shared-rejected ds.certificate-lifecycle-register-describe-deregister ds.conditional-forwarder-duplicate-rejected ds.create-conditional-forwarder-duplicate-rejected ds.conditional-forwarder-ip-list-round-trip ds.create-trust-duplicate-rejected ds.create-trust-duplicate-remote-domain-rejected ds.describe-ldaps-settings-unknown-directory-rejected-directory-does-not-exist ds.describe-directories-empty-id-list-rejected-invalid-parameter ds.enable-ldaps-without-certificate-blocked
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.accept-shared-directory-already-shared-rejected|run#12|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: ds.certificate-lifecycle-register-describe-deregister|run#18|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: ds.conditional-forwarder-duplicate-rejected|run#12|AssertionError|assert message.startswith(f"Failed to create zone {remote_domain} on server ") and message.endswith(
Issue-Key: ds.create-conditional-forwarder-duplicate-rejected|run#9|AssertionError|assert message.startswith(f"Failed to create zone {remote_domain} on server ") and message.endswith(
Issue-Key: ds.conditional-forwarder-ip-list-round-trip|run#21|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: ds.create-trust-duplicate-rejected|run#10|AssertionError|assert message == (
Issue-Key: ds.create-trust-duplicate-remote-domain-rejected|run#8|AssertionError|assert message == (
Issue-Key: ds.describe-ldaps-settings-unknown-directory-rejected-directory-does-not-exist|run#10|AssertionError|assert control_msg == expected_control_msg, (
Issue-Key: ds.describe-directories-empty-id-list-rejected-invalid-parameter|run#7|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: ds.enable-ldaps-without-certificate-blocked|run#11|AssertionError|assert message == (
Issue-Commit: 855e46d3b98757a511fcacb47a7c9e35083e2051

# ds: eight refusal messages are emulator-authored instead of AWS's text (ShareDirectory, certificates, conditional forwarders, trusts, DescribeDirectories, EnableLDAPS)

> _Note: this issue was identified against the code base at commit `855e46d3b98757a511fcacb47a7c9e35083e2051`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ShareDirectory

## Description
Eight net-new behavioral root causes in the Directory Service (ds) control plane. In every one the emulator picks the RIGHT modeled error class and the right HTTP 400, but words the message itself instead of reproducing AWS's text. An error message is a requirement, not decoration: these are eight distinct message templates owned by eight distinct refusal sites, and a fixer must close ALL EIGHT before submitting.

Cause 1 of 8 — ShareDirectory's already-shared refusal is re-worded and leaks the consumer's shared-directory id.
AWS: Smithy: ShareDirectory models DirectoryAlreadySharedException (client, HTTP 400) — "The specified directory has already been shared with this Amazon Web Services account."; DirectoryId @pattern ^d-[0-9a-f]{10}$, ShareTarget.Id is the twelve-digit consumer account id. Message template — fixed text: "Directory " <directory id> " is already shared with Account " <account id> "." followed by the envelope's " : RequestId: " <request id>; variable parts: the directory id and the target account id, both taken verbatim from the request. The capital "Account", the terminating full stop, and the absence of any other identifier are all fixed text.
Emulator: answers DirectoryAlreadySharedException with "Directory d-003ba5fe5a is already shared with account 020000000000 as d-007740c861 : RequestId: ..." — lower-case "account", no terminating full stop, and an extra " as <shared directory id>" clause AWS never emits (the shared id is state the refusal must not disclose).
Why: a client that matches this message to tell "already shared with this account" apart from the other DirectoryAlreadySharedException cases never matches, and a caller echoing the message to a user shows an internal consumer-side id AWS keeps out of the refusal.
Fix: ShareDirectory's handler, the DirectoryAlreadySharedException branch of the duplicate-share check (services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/share_directory.rs, the format! at the `store.share_of(..)` guard, around line 244).
Traces: ds.accept-shared-directory-already-shared-rejected.

Cause 2 of 8 — the certificate-not-found message drops the "directoryId:" label.
AWS: Smithy: DescribeCertificate and DeregisterCertificate both model CertificateDoesNotExistException (client, HTTP 400) and both take DirectoryId (@pattern ^d-[0-9a-f]{10}$) and CertificateId (@pattern ^c-[0-9a-f]{10}$). Message template — fixed text: "Certificate " <certificate id> " does not exist for directoryId: " <directory id>, then " : RequestId: " <request id>; variable parts: the two ids from the request. The literal member label "directoryId:" is fixed text.
Emulator: "Certificate c-003ba5fe5a does not exist for directory d-003ba5fe5a : RequestId: ..." — "for directory" instead of "for directoryId:".
Why: the same wording is served after DeregisterCertificate removes a certificate, so a client asserting on the deregistered-then-read path (and anything parsing the id out after the "directoryId:" label) diverges from AWS.
Fix: the ds certificate not-found message builder shared by DescribeCertificate and DeregisterCertificate.
Traces: ds.certificate-lifecycle-register-describe-deregister.

Cause 3 of 8 — CreateConditionalForwarder's duplicate-zone refusal is an emulator sentence instead of AWS's DNS-server zone message.
AWS: Smithy: CreateConditionalForwarder models EntityAlreadyExistsException (client, HTTP 400); RemoteDomainName is @required with @pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$. Message template — fixed text: "Failed to create zone " <remote domain name> " on server " <the directory's DNS server, taken from the directory's state> ". A conditional forwarding zone already exists for that name. " then " : RequestId: " <request id>; variable parts: the remote domain from the request and the DNS server name from the directory's stored controllers. The refusal is phrased as a failure to create the ZONE on the directory's server; the directory id does not appear.
Emulator: "Directory d-003ba5fe5a already has a conditional forwarder for the remote domain 'corp.sim2real-cfdup-....test' : RequestId: ..." — an entirely different sentence, naming the directory id and quoting the domain, with no zone/server clause.
Why: a client distinguishing "this zone already exists on the domain controller" from other EntityAlreadyExistsException cases, or surfacing the failing DNS server to an operator, gets neither.
Fix: CreateConditionalForwarder's handler, the duplicate-forwarder refusal branch, plus whatever renders the directory's DNS server name into refusals.
Traces: ds.conditional-forwarder-duplicate-rejected, ds.create-conditional-forwarder-duplicate-rejected.

Cause 4 of 8 — UpdateConditionalForwarder's missing-zone refusal is re-worded.
AWS: Smithy: UpdateConditionalForwarder models EntityDoesNotExistException (client, HTTP 400). Message template — fixed text: "The zone " <remote domain name> " was not found on the server." then " : RequestId: " <request id>; variable part: the remote domain name from the request, UNQUOTED.
Emulator: "Conditional forwarder: 'sim2real-forwarder.example.org' does not exist. : RequestId: ..." — different sentence, and the domain is wrapped in single quotes.
Why: this is the refusal a caller sees when it updates a forwarder that DeleteConditionalForwarder already removed; code that matches AWS's "was not found on the server" text to decide "recreate instead of update" takes the wrong branch.
Fix: UpdateConditionalForwarder's handler, the forwarder-lookup failure branch (it must not reuse a generic "<entity>: '<id>' does not exist." builder).
Traces: ds.conditional-forwarder-ip-list-round-trip.

Cause 5 of 8 — CreateTrust's duplicate-trust refusal is an emulator sentence instead of AWS's text.
AWS: Smithy: CreateTrust models EntityAlreadyExistsException (client, HTTP 400); DirectoryId @pattern ^d-[0-9a-f]{10}$, RemoteDomainName @required. Message template — fixed text: "Trust already exists for directory: " <directory id> " and remote domain: " <remote domain name> "." then " : RequestId: " <request id>; variable parts: the directory id and remote domain from the request, both UNQUOTED.
Emulator: "Directory d-003ba5fe5a already has a trust relationship with the remote domain 'sim2real-trust.example.test' : RequestId: ..." — different sentence, quoted domain, no terminating full stop.
Why: callers that retry or reconcile trusts by matching this message mis-classify the refusal; the divergence appears on every duplicate CreateTrust, for both an explicit and a generated remote domain.
Fix: CreateTrust's handler, the duplicate-remote-domain refusal branch.
Traces: ds.create-trust-duplicate-rejected, ds.create-trust-duplicate-remote-domain-rejected.

Cause 6 of 8 — DescribeDirectories' unknown-id refusal reuses the LDAPS-family wording instead of its own.
AWS: Smithy: DescribeDirectories models EntityDoesNotExistException (client, HTTP 400) and takes a DirectoryIds list of ids matching ^d-[0-9a-f]{10}$. Message template — fixed text: "Invalid directory ID(s): " <comma-separated unknown ids> "." then " : RequestId: " <request id>; variable part: the unknown id(s) from the request. This is deliberately NOT the LDAPS family's text: DescribeLDAPSSettings' DirectoryDoesNotExistException uses "Directory: '<id>' does not exist." and the emulator gets that one right, which is what makes the collision visible.
Emulator: DescribeDirectories(DirectoryIds=[<unknown id>]) answers the right code with "Directory: 'd-58b71cf0b5' does not exist. : RequestId: ..." — the LDAPS message, not DescribeDirectories'.
Why: the two families are the one signal a caller has for "this id is not in my inventory" versus "this directory cannot serve LDAPS"; collapsing their text erases the distinction the error codes are drawing.
Fix: DescribeDirectories' handler, the unknown-id refusal branch — it must build "Invalid directory ID(s): ..." rather than call the shared no-such-directory builder the LDAPS operations use.
Traces: ds.describe-ldaps-settings-unknown-directory-rejected-directory-does-not-exist.

Cause 7 of 8 — DescribeDirectories' empty-DirectoryIds refusal is re-worded.
AWS: Smithy: DescribeDirectories.DirectoryIds is an optional list and the operation models InvalidParameterException (client, HTTP 400). An explicitly EMPTY list is refused (omitting the member succeeds and returns the full inventory). Message template — fixed text: "If specified, DirectoryIds must have at least one DirectoryId." then " : RequestId: " <request id>; no variable parts.
Emulator: right code, message "DirectoryIds must not be an empty list : RequestId: ...".
Why: a caller asserting on AWS's sentence (the only thing that tells it the filter member, not the directory, was the problem) does not match.
Fix: DescribeDirectories' input validation, the empty-DirectoryIds branch.
Traces: ds.describe-directories-empty-id-list-rejected-invalid-parameter.

Cause 8 of 8 — EnableLDAPS' no-certificate refusal is re-worded and names the directory.
AWS: Smithy: EnableLDAPS models NoAvailableCertificateException (client, HTTP 400); DirectoryId @pattern ^d-[0-9a-f]{10}$, Type is the LDAPSType enum. Message template — fixed text, with NO variable parts: "Cannot enable LDAPS since there is no valid certificate available." then " : RequestId: " <request id>. The directory id does not appear in it.
Emulator: "LDAPS cannot be enabled for directory d-003ba5fe5a because at least one valid ClientLDAPS certificate must be registered in the system : RequestId: ..." — an emulator-authored sentence that interpolates the directory id into a message AWS keeps constant.
Why: clients matching AWS's constant text to prompt "register a certificate first" never match, and the id-bearing variant makes the message unmatchable by equality at all.
Fix: EnableLDAPS' handler, the NoAvailableCertificateException branch — emit the constant message.
Traces: ds.enable-ldaps-without-certificate-blocked.

Related open issues that do NOT cover these (none carries any of the check ids above): 20261029T180805Z-ds-directory-service-response-fidelity-input-validation-skipped-or.md records the same class of defect for VerifyTrust/DeleteTrust not-found wording, and 20261031T161124Z-ds-createcomputer-s-duplicate-name-entityalreadyexistsexception-message-is.md for CreateComputer's duplicate-name message; the eight templates above are different refusal sites.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against this checkout (ds: 30/155 passed). Each line is expected (AWS) vs actual (emulator); the RequestId is per-run and is not part of any requirement.

1) ds.accept-shared-directory-already-shared-rejected — ShareDirectory, code DirectoryAlreadySharedException matches.
   expected: "Directory d-003ba5fe5a is already shared with Account 020000000000. : RequestId: <rid>"
   actual:   "Directory d-003ba5fe5a is already shared with account 020000000000 as d-007740c861 : RequestId: <rid>"
   Fixed text diverging: "Account" vs "account", the missing ".", and the added " as <shared directory id>". Values from the request: the directory id and the account id; d-007740c861 is emulator state.

2) ds.certificate-lifecycle-register-describe-deregister — DescribeCertificate/DeregisterCertificate after deregistration, code CertificateDoesNotExistException matches.
   expected: "Certificate c-003ba5fe5a does not exist for directoryId: d-003ba5fe5a : RequestId: <rid>"
   actual:   "Certificate c-003ba5fe5a does not exist for directory d-003ba5fe5a : RequestId: <rid>"
   Fixed text diverging: "for directoryId:" vs "for directory". The two ids are request values.

3) ds.conditional-forwarder-duplicate-rejected and ds.create-conditional-forwarder-duplicate-rejected — CreateConditionalForwarder, code EntityAlreadyExistsException matches.
   expected: starts "Failed to create zone <remote domain> on server " and ends ". A conditional forwarding zone already exists for that name.  : RequestId: <rid>" (the server name between them is directory state the check does not pin).
   actual:   "Directory d-003ba5fe5a already has a conditional forwarder for the remote domain '<remote domain>' : RequestId: <rid>"

4) ds.conditional-forwarder-ip-list-round-trip — UpdateConditionalForwarder on a deleted forwarder, code EntityDoesNotExistException matches.
   expected: "The zone sim2real-forwarder.example.org was not found on the server. : RequestId: <rid>"
   actual:   "Conditional forwarder: 'sim2real-forwarder.example.org' does not exist. : RequestId: <rid>"

5) ds.create-trust-duplicate-rejected and ds.create-trust-duplicate-remote-domain-rejected — CreateTrust, code EntityAlreadyExistsException matches.
   expected: "Trust already exists for directory: <directory id> and remote domain: <remote domain>. : RequestId: <rid>"
   actual:   "Directory d-003ba5fe5a already has a trust relationship with the remote domain '<remote domain>' : RequestId: <rid>"

6) ds.describe-ldaps-settings-unknown-directory-rejected-directory-does-not-exist — the DescribeLDAPSSettings leg (DirectoryDoesNotExistException + "Directory: '<id>' does not exist.") PASSES; the DescribeDirectories control fails.
   expected: "Invalid directory ID(s): d-58b71cf0b5. : RequestId: <rid>"
   actual:   "Directory: 'd-58b71cf0b5' does not exist. : RequestId: <rid>"

7) ds.describe-directories-empty-id-list-rejected-invalid-parameter — DescribeDirectories(DirectoryIds=[]), code InvalidParameterException matches.
   expected: "If specified, DirectoryIds must have at least one DirectoryId. : RequestId: <rid>"
   actual:   "DirectoryIds must not be an empty list : RequestId: <rid>"

8) ds.enable-ldaps-without-certificate-blocked — EnableLDAPS(Type=Client) on an Active directory with ListCertificates == [], code NoAvailableCertificateException matches.
   expected: "Cannot enable LDAPS since there is no valid certificate available. : RequestId: <rid>"
   actual:   "LDAPS cannot be enabled for directory d-003ba5fe5a because at least one valid ClientLDAPS certificate must be registered in the system : RequestId: <rid>"

Model evidence (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json): ShareDirectory models DirectoryAlreadySharedException/InvalidTargetException/EntityDoesNotExistException (all client, 400); DescribeCertificate models CertificateDoesNotExistException; CreateConditionalForwarder models EntityAlreadyExistsException; UpdateConditionalForwarder models EntityDoesNotExistException; CreateTrust models EntityAlreadyExistsException; DescribeDirectories models EntityDoesNotExistException and InvalidParameterException; DescribeLDAPSSettings models DirectoryDoesNotExistException; EnableLDAPS models NoAvailableCertificateException. Code evidence for cause 1: src/ops/share_directory.rs builds the already-shared message with format!("Directory {id} is already shared with account {account} as {shared}") (~line 244).

## Steps to Reproduce
1. Clone this checkout and replay suite simparity-behavioral-aws-v1 against the booted emulator.
2. Cause 1: run ds.accept-shared-directory-already-shared-rejected — CreateMicrosoftAD, ShareDirectory(ShareMethod=HANDSHAKE, ShareTarget={Id:<account>,Type:ACCOUNT}), then repeat the identical ShareDirectory; read Error.Message.
3. Cause 2: run ds.certificate-lifecycle-register-describe-deregister — RegisterCertificate a self-signed ClientLDAPS PEM on an Active directory, DeregisterCertificate it, then DescribeCertificate the same CertificateId.
4. Cause 3: run ds.create-conditional-forwarder-duplicate-rejected — CreateConditionalForwarder for a remote domain twice on one Active Microsoft AD.
5. Cause 4: run ds.conditional-forwarder-ip-list-round-trip — CreateConditionalForwarder, DeleteConditionalForwarder, then UpdateConditionalForwarder for the same RemoteDomainName.
6. Cause 5: run ds.create-trust-duplicate-rejected — CreateTrust to one remote domain twice from one Active Microsoft AD.
7. Cause 6: run ds.describe-ldaps-settings-unknown-directory-rejected-directory-does-not-exist — DescribeDirectories(DirectoryIds=[<well-formed id no directory holds>]).
8. Cause 7: run ds.describe-directories-empty-id-list-rejected-invalid-parameter — DescribeDirectories(DirectoryIds=[]).
9. Cause 8: run ds.enable-ldaps-without-certificate-blocked — EnableLDAPS(DirectoryId=<Active directory with no registered certificate>, Type=Client).
10. In each case the error code and HTTP 400 are right and only Error.Message diverges; compare against the AWS templates quoted above.
