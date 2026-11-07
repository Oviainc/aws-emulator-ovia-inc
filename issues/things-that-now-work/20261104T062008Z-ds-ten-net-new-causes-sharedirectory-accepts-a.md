Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.share-directory-with-owner-account-rejected ds.shared-directory-lifecycle-transitions ds.register-duplicate-certificate-rejected ds.certificate-register-deregister-lifecycle ds.add-tags-invalid-tag-key-rejected ds.ds-data-access-must-be-enabled-before-the-data-plane-answers ds.accept-shared-directory-nonexistent-rejected ds.add-region-duplicate-rejected ds.reset-user-password-unknown-user-rejected ds.enable-sso-missing-permission-denied ds.e2e-trust-schema-extension-flow ds.state-machine-trust-lifecycle ds.trust-lifecycle-create-verify-delete ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust ds.alias-reassignment-rejected ds.ds-data-list-users-pagination-is-exact-and-reaches-the-token-layer ds.reset-user-password-enables-the-ds-data-user-it-names ds.certificate-registration-commonname-round-trip
Issue-Key: ds.share-directory-with-owner-account-rejected|_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ds.shared-directory-lifecycle-transitions|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {result!r}")
Issue-Key: ds.register-duplicate-certificate-rejected|_expect_client_error#7|AssertionError|raise AssertionError(f"expected AWS to reject the call, but it succeeded with {result!r}")
Issue-Key: ds.certificate-register-deregister-lifecycle|_error_code_and_message#8|AssertionError|raise AssertionError("expected the call to fail, but it succeeded")
Issue-Key: ds.add-tags-invalid-tag-key-rejected|_error_code_and_message#4|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with {resp!r}")
Issue-Key: ds.ds-data-access-must-be-enabled-before-the-data-plane-answers|_refusal#10|AssertionError|raise AssertionError(
Issue-Key: ds.accept-shared-directory-nonexistent-rejected|run#12|AssertionError|assert code == "EntityDoesNotExistException", f"expected EntityDoesNotExistException, got {code!r}"
Issue-Key: ds.add-region-duplicate-rejected|run#5|AssertionError|assert code == "UnsupportedOperationException", (
Issue-Key: ds.reset-user-password-unknown-user-rejected|run#4|AssertionError|assert code == "UserDoesNotExistException", f"expected UserDoesNotExistException, got {code}"
Issue-Key: ds.enable-sso-missing-permission-denied|run#10|AssertionError|assert code == "AuthenticationFailedException", f"expected AuthenticationFailedException, got {code}"
Issue-Key: ds.e2e-trust-schema-extension-flow|run#11|AssertionError|assert _trust_state() == "Failed", (
Issue-Key: ds.state-machine-trust-lifecycle|_wait_for_trust_state#8|Timeout|time.sleep(interval_s)
Issue-Key: ds.trust-lifecycle-create-verify-delete|_wait_for_trust_state#11|Timeout|time.sleep(interval_s)
Issue-Key: ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: ds.alias-reassignment-rejected|run#9|AssertionError|assert re.fullmatch(r"[A-Za-z0-9.-]+(:\d+)?", access_url), (
Issue-Key: ds.ds-data-list-users-pagination-is-exact-and-reaches-the-token-layer|run#22|AssertionError|assert page.get("Realm") == fqdn, (
Issue-Key: ds.reset-user-password-enables-the-ds-data-user-it-names|run#14|AssertionError|assert minted.get("Enabled") is False, (
Issue-Key: ds.certificate-registration-commonname-round-trip|_poll_for_client_error#18|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ds: ten net-new causes — ShareDirectory accepts a self-share, RegisterCertificate accepts ClientCertAuth, AddTagsToResource accepts an aws: key, ds-data answers with data access disabled, four refusal paths are 501 stubs, trusts never fail, AccessUrl carries a scheme, and ds-data users carry a fixed realm and land enabled

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ShareDirectory

## Description
Second bundle of net-new ds root causes from the same behavioral replay (simparity-behavioral-aws-v1), after skipping every failure the report marks ALREADY FILED or OWNED. These TEN causes are about requests the emulator ACCEPTS that AWS refuses, refusal paths that are still scaffold stubs, and state/representation that never reaches AWS's outcome. A fixer must close ALL TEN causes before submitting. Related open issues naming the same ground for OTHER checks (they carry no replay key for the checks here, so they do not cover them): 20261102T012359Z-ds-the-trust-state-machine-never-reaches-failed.md, 20261023T134711Z-ds-data-user-objects-are-not-real-active.md, 20261103T100504Z-ds-directory-service-certificate-and-directory-lookups-unvalidated-shortname.md.

Cause 1 of 10: ShareDirectory accepts the directory owner's OWN account as the share target.
AWS: Smithy ShareDirectory models InvalidTargetException ('The specified shared target is not valid.', client, HTTP 400) with members Message and RequestId. Recorded behavior: ShareDirectory with ShareTarget={Id: <the directory owner's account id>, Type: ACCOUNT} is refused 400 InvalidTargetException with the message template 'The Target AWS account ID <account id> you specified cannot be the same as the directory owner AWS account ID.' (fixed text; the variable part is the caller's own account id) and NO shared-directory record is created, while the identical call naming a different account still succeeds.
Emulator: answers HTTP 200 with a SharedDirectoryId and records the self-share.
Why: a client's guard against mis-targeted sharing never fires, and the directory ends up with a share record AWS would never create — the subsequent DescribeSharedDirectories inventory is wrong for every consumer.
Fix: ShareDirectory's request validation (target-vs-owner comparison) before the handler mutates share state.
Traces: ds.share-directory-with-owner-account-rejected, ds.shared-directory-lifecycle-transitions.

Cause 2 of 10: RegisterCertificate accepts Type=ClientCertAuth on a Managed Microsoft AD directory.
AWS: Smithy RegisterCertificate models UnsupportedOperationException (client, 400) alongside CertificateAlreadyExistsException, InvalidCertificateException, DirectoryDoesNotExistException, DirectoryUnavailableException. Recorded behavior: RegisterCertificate(Type='ClientCertAuth') against an Active AWS Managed Microsoft AD is refused 400 UnsupportedOperationException with the fixed text 'Client authentication operations are not supported for this Directory Type.' and ListCertificates stays empty.
Emulator: answers HTTP 200 with a CertificateId (the certificate is registered), so the refusal is unreachable.
Why: a client that uses the refusal to detect that client-certificate authentication is unavailable for this directory type instead believes it enabled smart-card auth; and the certificate inventory gains an entry AWS would not hold.
Fix: RegisterCertificate's handler — gate Type=ClientCertAuth on directory type and refuse with UnsupportedOperationException before storing anything.
Traces: ds.register-duplicate-certificate-rejected, ds.certificate-register-deregister-lifecycle.

Cause 3 of 10: AddTagsToResource accepts a reserved 'aws:'-prefixed tag key.
AWS: Smithy AddTagsToResource models InvalidParameterException (client, 400) and TagLimitExceededException. Recorded behavior: a tag whose Key begins with 'aws:' is refused 400 InvalidParameterException with the fixed text 'Adding or removing system tags (with prefix aws:) is not supported.' and no such tag is stored, while an ordinary key on the same directory is accepted.
Emulator: answers HTTP 200 (empty body) and stores the reserved tag.
Why: a caller can write into the AWS-reserved tag namespace on the emulator and its tag-governance tests pass where AWS rejects the same call.
Fix: AddTagsToResource's tag-key validation, before the state write.
Traces: ds.add-tags-invalid-tag-key-rejected.

Cause 4 of 10: the ds-data plane answers for a directory whose data access was never enabled.
AWS: Smithy (directoryservicedata) CreateUser models AccessDeniedException (client, HTTP 403) whose members are Message and a Reason enum (the gate case is the DATA_DISABLED reason). Recorded behavior: any ds-data call against a directory on which EnableDirectoryDataAccess was never called is refused 403 AccessDeniedException with the fixed text 'DS Data feature is not enabled on this directory. This feature must first be enabled by calling the EnableDirectoryDataAccess API.'
Emulator: ds-data CreateUser answers HTTP 200 with DirectoryId/SAMAccountName/SID — the data-access gate is not enforced at all.
Why: the whole access-control boundary between the ds control plane and the ds-data plane is absent: a caller that never enabled the feature still reads and writes directory objects, and its negative test for the gate passes against the emulator and fails against AWS.
Fix: the ds-data request path's directory-data-access gate (shared pre-handler check on every ds-data operation), emitting AccessDeniedException with the DATA_DISABLED Reason.
Traces: ds.ds-data-access-must-be-enabled-before-the-data-plane-answers.

Cause 5 of 10: four refusal paths answer HTTP 501 'NotImplemented' because the operation is still a scaffold stub, so modelled refusals are unreachable. The checkout shows the SCAFFOLDED marker in services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/{accept_shared_directory.rs, add_region.rs, reset_user_password.rs, enable_sso.rs}.
AWS: (a) AcceptSharedDirectory models EntityDoesNotExistException (client, 400); a SharedDirectoryId no handshake created is refused with 'Directory: \'<shared directory id>\' does not exist.' (fixed text, variable part the id from the request) and no shared-directory record appears. (b) AddRegion models UnsupportedOperationException, DirectoryAlreadyInRegionException, RegionLimitExceededException (client, 400); AddRegion naming the directory's OWN home Region is refused UnsupportedOperationException. (c) ResetUserPassword models UserDoesNotExistException (client, 400); a UserName absent from the directory is refused with 'User: <user name> does not exist.' while resetting the directory's real Admin account succeeds. (d) EnableSso models AuthenticationFailedException and InsufficientPermissionsException (client, 400); EnableSso with credentials lacking the required directory permission is refused AuthenticationFailedException.
Emulator: each answers HTTP 501 with Error.Code 'NotImplemented' and the message 'operation <Op> is scaffolded but not implemented in this emulator'.
Why: a 501 is not a modelled ds error at all — client SDK error handling has no branch for it, retries treat it as a server fault, and none of these four refusal semantics can be exercised.
Fix: implement the four handlers named above, each producing the modelled refusal (and the success path the positive controls need). Note: an issue naming the EnableSso stub and its handler file is already filed for you (it was raised from ds.disable-sso-authentication-failed-rejected); the EnableSso item here is listed only so the check below is accounted for — the net-new implementation work is AcceptSharedDirectory, AddRegion and ResetUserPassword.
Traces: ds.accept-shared-directory-nonexistent-rejected, ds.add-region-duplicate-rejected, ds.reset-user-password-unknown-user-rejected, ds.enable-sso-missing-permission-denied.

Cause 6 of 10: the trust state machine can never reach Failed — a trust to an unreachable/unresolvable remote domain settles Created.
AWS: Smithy TrustState enum = Creating, Created, Verifying, VerifyFailed, Verified, Updating, UpdateFailed, Updated, Deleting, Deleted, Failed; Trust.TrustStateReason is 'The reason for the TrustState'. Recorded behavior: CreateTrust to a domain with no reachable peer (e.g. a .invalid or freshly-minted .test name) is ACCEPTED, then driven Creating -> Failed with a non-empty TrustStateReason (the recording's reason text is 'An internal service error has been encountered while creating the trust. Please retry the operation.'), never Verified and never Created; VerifyTrust on that trust is then refused 400 InvalidParameterException naming the state it refused from ('Cannot verify trust in state: <state>').
Emulator: the remote domain is never resolved, so the trust transitions Creating -> Created unconditionally and no TrustStateReason is ever produced. Checks that assert the terminal state see 'Created'; checks that POLL for Failed never converge and are killed at the harness 60 s budget.
Why: the only outcome a trust can have against this emulator is success — a client cannot diagnose a trust that did not establish, its failure branch is dead code, and a poll for a terminal failure hangs.
Fix: the trust branch of this service's state machine (CreateTrust handler + the clock advance that moves trust states) must settle an unreachable remote domain into Failed with a TrustStateReason, and VerifyTrust must refuse from non-verifiable states.
Traces: ds.e2e-trust-schema-extension-flow, ds.state-machine-trust-lifecycle, ds.trust-lifecycle-create-verify-delete, ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust.

Cause 7 of 10: DirectoryDescription.AccessUrl is emitted with a URL scheme instead of a bare host.
AWS: Smithy documents AccessUrl as 'The access URL for the directory, such as http://<alias>.awsapps.com', but recorded behavior (what the check measures on real AWS) is a BARE HOST: after CreateAlias, DescribeDirectories reports AccessUrl matching ^[A-Za-z0-9.-]+(:\\d+)?$ and leading with the alias, i.e. '<alias>.awsapps.com' with no scheme and no path.
Emulator: returns 'http://<alias>.awsapps.com'.
Why: a client that concatenates a scheme or uses the value as a Host header / hostname builds 'http://http://<alias>.awsapps.com' or fails hostname validation.
Fix: the DescribeDirectories serializer / the CreateAlias handler's AccessUrl composition — store and emit the host only.
Traces: ds.alias-reassignment-rejected.

Cause 8 of 10: ds-data user objects carry a hard-coded realm instead of the directory's own domain.
AWS: Smithy (directoryservicedata) ListUsers and DescribeUser both return Realm (@pattern ^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$), and User carries DistinguishedName and UserPrincipalName. Recorded behavior: the Realm every page echoes — and the realm embedded in UserPrincipalName and DistinguishedName — is the DIRECTORY's own fully-qualified domain name, the Name the directory was created with.
Emulator: every ds-data read reports Realm 'corp.example.com', UserPrincipalName '<user>@corp.example.com' and DistinguishedName 'CN=<user>,CN=Users,DC=corp,DC=example,DC=com' regardless of the directory's actual domain.
Why: a client that derives LDAP/Kerberos identities from these fields addresses a domain that does not exist, and a per-directory assertion on Realm fails for every directory whose name is not 'corp.example.com'.
Fix: the ds-data user projection/serializer — derive Realm, UserPrincipalName and DistinguishedName from the directory record's domain name.
Traces: ds.ds-data-list-users-pagination-is-exact-and-reaches-the-token-layer.

Cause 9 of 10: a ds-data CreateUser that sets no password lands Enabled=true.
AWS: Smithy User.Enabled is 'Indicates whether the user account is active'; CreateUser takes no password member at all. Recorded behavior: a user created through ds-data has no password, so Active Directory leaves the account INACTIVE — DescribeUser reports Enabled=false until a ds ResetUserPassword sets a password, which is what flips it to true.
Emulator: DescribeUser reports Enabled=true immediately after CreateUser, so the enable transition cannot be observed.
Why: a client that checks Enabled before handing the account to a user sees an account AD would refuse authentication for as ready, and the reset-then-enable workflow has nothing to flip.
Fix: the ds-data CreateUser handler (store the account disabled) together with the ds ResetUserPassword handler that must enable it.
Traces: ds.reset-user-password-enables-the-ds-data-user-it-names.

Cause 10 of 10: a registered ClientLDAPS certificate never settles into a terminal certificate state.
AWS: Smithy RegisterCertificate returns a CertificateId (@pattern ^c-[0-9a-f]{10}$); recorded behavior: DescribeCertificate on it reaches a terminal state (Registered, or RegisterFailed) within a bounded wait, and the parsed CommonName of the submitted PEM is reported back.
Emulator: the check is cut off at the harness 60 s cap with no response of its own; the only bounded poll it reaches after the directory is Active is the certificate-state poll. Hypothesis: a ClientLDAPS certificate is left in the transient Registering state forever, so no terminal state is ever observed (this reading is not stated by the model or a response body — the grounded fact is the timeout).
Why: every LDAPS-certificate workflow blocks — a client waiting for Registered before enabling LDAPS waits forever, and the parsed-PEM representation (CommonName, expiry) is never asserted.
Fix: the certificate state machine of this service — the RegisterCertificate handler plus the clock advance that moves certificate states must drive a registered certificate to a terminal state and expose the PEM's parsed CommonName.
Traces: ds.certificate-registration-commonname-round-trip.

## Evidence
Suite simparity-behavioral-aws-v1 against this checkout: Total 56/178 passed (31.5%); 95 mismatched, 27 errored, 0 skipped, 0 unloadable. Per service: ds 53/155 (34%, 102 failed), ds__ec2 3/21 (14%, 18 failed), ds__logs 0/1, ds__sns 0/1.

Expected vs actual, per cause (verbatim from the replay report):
1) ds.share-directory-with-owner-account-rejected — 'expected the call to be rejected, but it succeeded: {SharedDirectoryId: d-00b2db9268, HTTPStatusCode: 200}'; the check expects InvalidTargetException 'The Target AWS account ID <own account> you specified cannot be the same as the directory owner AWS account ID.' ds.shared-directory-lifecycle-transitions fails the same way ('expected a ClientError, call succeeded with {SharedDirectoryId: ...}').
2) ds.register-duplicate-certificate-rejected — 'expected AWS to reject the call, but it succeeded with {CertificateId: c-003ba5fe5a, HTTPStatusCode: 200}'; expected UnsupportedOperationException 'Client authentication operations are not supported for this Directory Type. : RequestId: <id>'. ds.certificate-register-deregister-lifecycle: 'expected the call to fail, but it succeeded' on the same RegisterCertificate(Type=ClientCertAuth) call.
3) ds.add-tags-invalid-tag-key-rejected — 'expected a ClientError, call succeeded with {HTTPStatusCode: 200, content-length: 2}'; expected InvalidParameterException 'Adding or removing system tags (with prefix aws:) is not supported. : RequestId: <id>'.
4) ds.ds-data-access-must-be-enabled-before-the-data-plane-answers — 'CreateUser against a directory whose data access has never been enabled SUCCEEDED where real AWS refuses it; the response was {HTTPStatusCode: 200, DirectoryId: d-003ba5fe5a, SAMAccountName: sim2realGated, SID: S-1-5-21-...}'; the check's gate message constant is 'DS Data feature is not enabled on this directory. This feature must first be enabled by calling the EnableDirectoryDataAccess API.'
5) ds.accept-shared-directory-nonexistent-rejected — 'expected EntityDoesNotExistException, got NotImplemented'; ds.add-region-duplicate-rejected — "AddRegion on d-003ba5fe5a's own home Region 'us-west-2' returned 'NotImplemented', expected UnsupportedOperationException"; ds.reset-user-password-unknown-user-rejected — 'expected UserDoesNotExistException, got NotImplemented'; ds.enable-sso-missing-permission-denied — 'expected AuthenticationFailedException, got NotImplemented'. Checkout: the four handler files carry the '//! SCAFFOLDED, NOT IMPLEMENTED' marker (accept_shared_directory.rs, add_region.rs, reset_user_password.rs, enable_sso.rs under services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/).
6) ds.e2e-trust-schema-extension-flow — 'a one-way trust to a domain with no reachable peer must deterministically fail, not land on Created'. ds.state-machine-trust-lifecycle, ds.trust-lifecycle-create-verify-delete and ds.ds-trust-to-an-unresolvable-remote-domain-fails-and-leaves-no-trust each report 'Error: Check timed out after 60.0s' while polling DescribeTrusts for TrustState=Failed (ds.trust-lifecycle-create-verify-delete additionally asserts TrustStateReason 'An internal service error has been encountered while creating the trust. Please retry the operation.' and VerifyTrust refusal 'Cannot verify trust in state: Failed : RequestId: <id>').
7) ds.alias-reassignment-rejected — "AccessUrl is not a well-formed host: 'http://sim2realalias<run>.awsapps.com'" (the check asserts re.fullmatch(r'[A-Za-z0-9.-]+(:\\d+)?', access_url) and that it leads with the alias).
8) ds.ds-data-list-users-pagination-is-exact-and-reaches-the-token-layer — "expected every page to echo the directory's own realm, expected 'sim2real-behav-<run>.example.com', got 'corp.example.com' on page 1". The same fixed realm shows in the DescribeUser body recorded by ds.ds-data-user-create-describe-search-roundtrip: Realm 'corp.example.com', UserPrincipalName '<user>@corp.example.com', DistinguishedName 'CN=<user>,CN=Users,DC=corp,DC=example,DC=com'.
9) ds.reset-user-password-enables-the-ds-data-user-it-names — 'expected a ds-data CreateUser to land DISABLED (Enabled is False) so the reset below has something to flip, got Enabled=True'.
10) ds.certificate-registration-commonname-round-trip — 'Error: Check timed out after 60.0s'; the check registers a ClientLDAPS PEM and polls DescribeCertificate for {Registered, RegisterFailed} before comparing CommonName.

Model evidence: ds model — ShareDirectory errors include InvalidTargetException ('The specified shared target is not valid.', 400, members Message+RequestId); RegisterCertificate errors include UnsupportedOperationException; AddTagsToResource errors include InvalidParameterException; AcceptSharedDirectory errors include EntityDoesNotExistException; AddRegion errors include UnsupportedOperationException/DirectoryAlreadyInRegionException; ResetUserPassword errors include UserDoesNotExistException; EnableSso errors include AuthenticationFailedException/InsufficientPermissionsException; TrustState enum includes Failed and Trust.TrustStateReason is modelled; DirectoryDescription.AccessUrl doc shows a scheme-prefixed EXAMPLE while the recordings show a bare host. ds-data model — CreateUser errors = AccessDeniedException (403, members Message + Reason enum), ConflictException (409), ValidationException (400); ListUsers/DescribeUser return Realm; User.Enabled is 'Indicates whether the user account is active'.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite() to build, boot and replay against the emulator.
2. Cause 1: create an Active Managed Microsoft AD, then ShareDirectory(ShareMethod='HANDSHAKE', ShareTarget={Id: <caller's own account id>, Type: 'ACCOUNT'}) — expect 400 InvalidTargetException with the 'cannot be the same as the directory owner AWS account ID.' message and no new record in DescribeSharedDirectories; observe HTTP 200 and a SharedDirectoryId.
3. Cause 2: RegisterCertificate(DirectoryId=<Managed Microsoft AD>, CertificateData=<self-signed CA PEM>, Type='ClientCertAuth') — expect 400 UnsupportedOperationException 'Client authentication operations are not supported for this Directory Type.' and ListCertificates empty; observe HTTP 200 with a CertificateId.
4. Cause 3: AddTagsToResource(ResourceId=<directory>, Tags=[{Key:'aws:reserved-tag', Value:'x'}]) — expect 400 InvalidParameterException 'Adding or removing system tags (with prefix aws:) is not supported.'; observe HTTP 200 and the tag stored.
5. Cause 4: create a directory and WITHOUT calling EnableDirectoryDataAccess, call ds-data CreateUser on it — expect 403 AccessDeniedException with the DS-Data-not-enabled message and Reason DATA_DISABLED; observe HTTP 200 with a SID.
6. Cause 5: call AcceptSharedDirectory(SharedDirectoryId='d-9026a1b2c3'), AddRegion(<directory>, RegionName=<its own home region>), ResetUserPassword(<directory>, UserName='sim2real-no-such-user'), EnableSso(<directory>, UserName/Password lacking permission) — expect the modelled refusals above; observe HTTP 501 Error.Code 'NotImplemented'.
7. Cause 6: CreateTrust(RemoteDomainName='unreachable.sim2real.invalid', TrustDirection='Two-Way'|'One-Way: Outgoing') and poll DescribeTrusts — expect Creating -> Failed with a TrustStateReason; observe it settle Created (and a poll for Failed hang until the harness cap).
8. Cause 7: CreateAlias then DescribeDirectories — expect AccessUrl '<alias>.awsapps.com'; observe 'http://<alias>.awsapps.com'.
9. Cause 8: create a directory named <unique>.example.com, enable data access, ds-data CreateUser, then ListUsers/DescribeUser — expect Realm/UPN/DN built from <unique>.example.com; observe corp.example.com.
10. Cause 9: ds-data CreateUser then DescribeUser — expect Enabled=false; observe true. Then ds ResetUserPassword for that user and expect Enabled to become true.
11. Cause 10: RegisterCertificate(Type='ClientLDAPS') with a PEM whose CommonName is known, then poll DescribeCertificate — expect a terminal state (Registered/RegisterFailed) and the same CommonName; observe no terminal state within the budget.
12. After fixing, re-replay every check id in trace_ids.
