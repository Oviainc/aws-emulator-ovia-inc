Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.email-identity-lifecycle sesv2.email-identity-verification-info-separates-a-missing-dns-zone-from-a-pending-one sesv2.email-identity-inherits-verified-parent-domain-dkim sesv2.list-management-options-unknown-list-rejected-contact-auto-created sesv2.send-email-list-management-options-are-resolved-against-the-contact-list sesv2.import-job-invalid-data-source-rejected sesv2.v1-template-token-continues-the-v2-list
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|ListEmailIdentities|NotImplemented
Issue-Key: sesv2.email-identity-verification-info-separates-a-missing-dns-zone-from-a-pending-one|_await_verification_info#7|Timeout|time.sleep(_VERIFY_INTERVAL_S)
Issue-Key: sesv2.email-identity-inherits-verified-parent-domain-dkim|run#10|AssertionError|assert child_dkim["SigningEnabled"] is True, (
Issue-Key: sesv2.list-management-options-unknown-list-rejected-contact-auto-created|run#10|AssertionError|assert refused == expected_refused, (
Issue-Key: sesv2.send-email-list-management-options-are-resolved-against-the-contact-list|run#11|AssertionError|assert (valid_code, valid_message) == expected_rejected, (
Issue-Key: sesv2.import-job-invalid-data-source-rejected|run#6|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: sesv2|CreateTemplate|NotImplemented
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# sesv2: 7 net-new behavioral divergences — ListEmailIdentities unreachable on the SDK's URI, VerificationInfo never reported, child identities do not inherit parent DKIM, SendEmail ListManagementOptions is a 501 stub, wrong MessageRejected/validation message rules, v1 CreateTemplate stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEmailIdentities, GetEmailIdentity, CreateEmailIdentity, SendEmail, CreateImportJob, CreateTemplate

## Description
Seven distinct root causes in the SESv2 slice (one of them in the SES v1 crate that a sesv2 check drives). All seven must be closed before submitting; each is numbered with its own grounding checks.

=== Cause 1 of 7: ListEmailIdentities is not reachable on the URI AWS SDKs send ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) models ListEmailIdentities with output {EmailIdentities: IdentityInfoList, NextToken} and errors BadRequestException (400) / TooManyRequestsException (429); the checked-in model binds it to POST /v2/email/list-identities, while the AWS SDKs (botocore's sesv2 model, which every client and the CLI use) send GET /v2/email/identities and real AWS answers that request 200 with the identity list. Recorded behavior: the GET form is the live wire form.
Emulator: the request is refused 501 NotImplemented with the message "operation GET /v2/email/identities is not implemented in this emulator" — a router-level refusal, not the scaffold-stub refusal. The handler itself is fully written (src/ops/list_email_identities.rs: filtering, PageSize bound, opaque NextToken), so the behavior exists but is bound to a URI no client sends.
Why: no SDK, CLI or paginator can ever enumerate email identities; every list/paginate step of an identity workflow dies on a 501 that SDK retry logic treats as fatal, and the fully-implemented listing behavior is dead code.
Fix: the sesv2 service's HTTP route/model binding for ListEmailIdentities — bind the operation to the method+URI real clients use (GET /v2/email/identities) in addition to whatever the checked-in model declares; the handler needs no change.
Traces: sesv2.email-identity-lifecycle.

=== Cause 2 of 7: GetEmailIdentity never populates VerificationInfo ===
AWS: Smithy GetEmailIdentityResponse.VerificationInfo -> VerificationInfo {LastCheckedTimestamp, LastSuccessTimestamp, ErrorType: VerificationError, SOARecord}. The VerificationError documentation draws the line the check asserts: HOST_NOT_FOUND = "The queried hostname does not exist or was not reachable at the time of the request", TYPE_NOT_FOUND = "The queried hostname exists but does not have the requested type of DNS record". Recorded behavior: for a domain whose parent zone does not exist at all the member converges to ErrorType HOST_NOT_FOUND; for a domain under a resolvable zone whose DKIM CNAME was never published it converges to ErrorType TYPE_NOT_FOUND (with no SOARecord), and LastCheckedTimestamp is a live timestamp later than the identity's creation.
Emulator: GetEmailIdentity hard-codes the member absent — src/ops/get_email_identity.rs lines 209-211 return `verification_info: None` with the comment "this emulator probes nothing, so it reports nothing". The check polls GetEmailIdentity every 10s waiting for any VerificationInfo carrying an ErrorType and gets `None` on every poll, so it consumes its whole budget and the suite reports "Check timed out after 60.0s" instead of a value comparison.
Why: a client cannot distinguish "DNS name does not exist" from "record type missing" from "verification still pending": the only diagnostic SESv2 offers for a stuck domain identity is permanently absent, and any code branching on VerificationInfo.ErrorType takes the no-information path forever.
Fix: GetEmailIdentity's handler plus the identity state it reads (src/ops/get_email_identity.rs and the EmailIdentityRecord verification state in src/state.rs) — record a verification attempt per identity (a LastCheckedTimestamp from the live clock) and classify the failure into the modelled VerificationError values rather than returning the member unset.
Traces: sesv2.email-identity-verification-info-separates-a-missing-dns-zone-from-a-pending-one.

=== Cause 3 of 7: an address identity under a verified parent domain does not inherit the parent's DKIM attributes ===
AWS: Smithy DkimAttributes {SigningEnabled (boolean, default false), Status (DkimStatus: PENDING|SUCCESS|FAILED|TEMPORARY_FAILURE|NOT_STARTED), Tokens (DnsTokenList), ...}. Recorded behavior: an EMAIL_ADDRESS identity created under a domain identity that is already verified (DkimAttributes.Status == SUCCESS) carries no DKIM configuration of its own — GetEmailIdentity on the child mirrors the parent's SigningEnabled=true, Status=SUCCESS and the parent's exact Tokens list, both immediately after creation and on an independent re-read.
Emulator: the child is seeded with DKIM state of its own; GetEmailIdentity on the child reports SigningEnabled=false (assertion: "a child identity under a verified domain must inherit SigningEnabled=True; got False"). The parent in the same run reads Status=SUCCESS, so the two resources are not related at all.
Why: a client that verifies a domain once and then sends from per-user addresses under it sees every address as un-DKIM-signed and never-verified, so any gate on DkimAttributes.SigningEnabled/Status for an address identity fails although AWS would pass it.
Fix: the identity state's DKIM derivation and CreateEmailIdentity's handler (src/state.rs EmailIdentityRecord::dkim_attributes and src/ops/create_email_identity.rs) — an address identity's DkimAttributes must be resolved from its parent domain identity (domain and subdomain match) at read time, not stored as an independent pending set.
Traces: sesv2.email-identity-inherits-verified-parent-domain-dkim.

=== Cause 4 of 7: SendEmail refuses any request carrying ListManagementOptions with 501 NotImplemented ===
AWS: Smithy SendEmailRequest.ListManagementOptions -> ListManagementOptions {ContactListName (required), TopicName}; SendEmail's modelled errors include NotFoundException (404) and MessageRejected (400). Recorded behavior: SendEmail resolves ListManagementOptions against the contact lists — naming a contact list that does not exist is refused NotFoundException with the message "List with name: <ContactListName> doesn't exist." (fixed text "List with name: " + the request's ContactListName + " doesn't exist."), and a well-formed list/topic pair proceeds to the sender-identity gate.
Emulator: src/ops/send_email.rs lines 225-252 refuse the request before any of that, returning 501 NotImplemented 'operation SendEmail with ListManagementOptions is scaffolded but not implemented in this emulator' whenever the member is present (the same block also refuses FromEmailAddressIdentityArn, FeedbackForwardingEmailAddressIdentityArn, EndpointId, TenantName and ConfigurationOverrides).
Why: unsubscribe/list-management sends — the normal shape of any contact-list mailing — cannot be exercised at all, and the client gets an unretryable 501 where AWS returns either a 200 MessageId or a modelled 404/400 it can act on.
Fix: SendEmail's handler (src/ops/send_email.rs) — resolve ListManagementOptions against the contact-list state (unknown list -> NotFoundException with AWS's message rule; unknown topic -> the topic-specific refusal) and keep the resolution ahead of the FromEmailAddress identity gate, instead of refusing the member.
Traces: sesv2.list-management-options-unknown-list-rejected-contact-auto-created, sesv2.send-email-list-management-options-are-resolved-against-the-contact-list.

=== Cause 5 of 7: SendEmail's unverified-sender MessageRejected message omits the region clause ===
AWS: SendEmail error MessageRejected (400). Recorded behavior: the unverified-sender refusal message is "Email address is not verified. The following identities failed the check in region <REGION>: <address>" — fixed text "Email address is not verified. The following identities failed the check in region ", then the request's region token in upper case with hyphens kept (US-WEST-2 in the recording), then ": ", then the offending sender address taken from the request.
Emulator: src/ops/send_email.rs lines 301-306 build "Email address is not verified. The following identities failed the check: {from}" — the " in region <REGION>" clause is missing entirely. (Observed as the expected value of the same check that Cause 4 blocks: the check asserts the full AWS sentence, so it still fails on the message after the ListManagementOptions stub is implemented.)
Why: clients and test suites match SES's documented sentence to tell an identity-gate rejection apart from other MessageRejected causes, and operators read the region out of it; the truncated form breaks both.
Fix: SendEmail's refusal construction (src/ops/send_email.rs) — render the message from the request's region, not a region-less variant.
Traces: sesv2.send-email-list-management-options-are-resolved-against-the-contact-list.

=== Cause 6 of 7: CreateImportJob's pattern-constraint refusal interpolates the rejected value, AWS omits it ===
AWS: Smithy ImportDataSource.S3Url is required with @pattern ^s3:\/\/([^\/]+)\/(.*?([^\/]+)\/?)$; CreateImportJob models BadRequestException (400). Recorded behavior: the refusal message is "1 validation error detected: Value at 'importDataSource.s3Url' failed to satisfy constraint: Member must satisfy regular expression pattern: ^s3:\/\/([^\/]+)\/(.*?([^\/]+)\/?)$" — the fixed text is "1 validation error detected: Value at '" / "' failed to satisfy constraint: Member must satisfy regular expression pattern: ", the variable parts are the member path (importDataSource.s3Url) and the pattern; for this member AWS renders the value-less form and never echoes the submitted URL.
Emulator: the same message with the submitted value spliced in — "1 validation error detected: Value 'not-a-valid-s3-url' at 'importDataSource.s3Url' failed to satisfy constraint: ..." (the recorded run's literal value fills that slot; the defect is that the slot exists at all).
Why: a client asserting on, or logging, SES's validation sentence sees a different message shape than AWS produces, and the emulator echoes caller-supplied content into an error message where AWS deliberately does not.
Fix: the constraint-validation message renderer the request walk uses for this operation's input (reached before src/ops/create_import_job.rs's handler) — emit the value-less "Value at '<path>'" form for this member's pattern constraint.
Traces: sesv2.import-job-invalid-data-source-rejected.

=== Cause 7 of 7: SES v1 CreateTemplate is an unimplemented scaffold stub, so v1/v2 shared template pagination cannot be reached ===
AWS: SES v1 CreateTemplate creates an email template whose name is then visible to SESv2 ListEmailTemplates; recorded behavior: a NextToken minted by the v1 ListTemplates call is accepted by v2 ListEmailTemplates and continues the SAME underlying list (the v2 page that follows does not repeat what v1's first page returned).
Emulator: CreateTemplate answers 501 NotImplemented — "operation CreateTemplate is scaffolded but not implemented in this emulator"; services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_template.rs still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and an Err(..) body. The sesv2-owned check dies on its first setup call, so nothing about the shared cursor space is graded.
Why: templates created through the v1 API are invisible to v2 (and cannot be created at all), so any client that still uses SES v1 template management against the emulator is dead on arrival, and the v1/v2 pagination contract is untestable.
Fix: implement the handler in services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_template.rs over the template state shared with the sesv2 slice (and delete the SCAFFOLDED marker line), so v1 and v2 template listings share one store and one cursor space.
Traces: sesv2.v1-template-token-continues-the-v2-list.

Note on neighbouring filed work: the open issues 20261023T183615Z-sesv2-is-unreachable-every-request-is-refused-ambiguousservice.md and 20261023T183615Z-sesv2-sns-sqs-all-seven-ses-send-event.md describe a pre-dispatch 'AmbiguousService' router refusal; in this replay requests do reach the sesv2 handlers (real validation and modelled errors come back), so none of the seven causes above is that router defect.

## Evidence
Suite: 3/113 passed. Surviving (not already-filed, not owned) sesv2 clusters:

1) sesv2.email-identity-lifecycle — botocore ClientError: An error occurred (NotImplemented) when calling the ListEmailIdentities operation: "operation GET /v2/email/identities is not implemented in this emulator". Expected: HTTP 200 with EmailIdentities/NextToken. Handler exists at src/ops/list_email_identities.rs (complete) but the checked-in model binds POST /v2/email/list-identities.

2) sesv2.email-identity-verification-info-... — "Check timed out after 60.0s" while polling GetEmailIdentity every 10s for VerificationInfo carrying an ErrorType. Emulator source: src/ops/get_email_identity.rs:209-211 `verification_info: None` unconditionally. Expected: VerificationInfo.ErrorType=HOST_NOT_FOUND for a name under a nonexistent TLD, TYPE_NOT_FOUND for a name under a resolvable zone with no DKIM CNAME, plus a live LastCheckedTimestamp.

3) sesv2.email-identity-inherits-verified-parent-domain-dkim — AssertionError: "a child identity under a verified domain must inherit SigningEnabled=True; got False" (parent read DkimAttributes.Status == SUCCESS in the same run; the check also requires child Status == SUCCESS and child Tokens == parent Tokens on re-read).

4) sesv2.list-management-options-unknown-list-rejected-contact-auto-created — AssertionError: expected ('NotFoundException', "List with name: <contact-list-name> doesn't exist."); got ('NotImplemented', 'operation SendEmail with ListManagementOptions is scaffolded but not implemented in this emulator'). Same stub hit by sesv2.send-email-list-management-options-are-resolved-against-the-contact-list, which expected the identity gate instead. Emulator source: src/ops/send_email.rs:225-252.

5) sesv2.send-email-list-management-options-... — expected ('MessageRejected', 'Email address is not verified. The following identities failed the check in region US-WEST-2: <sender>'); emulator source src/ops/send_email.rs:301-306 formats 'Email address is not verified. The following identities failed the check: {from}' (no region clause).

6) sesv2.import-job-invalid-data-source-rejected — AssertionError: expected message "1 validation error detected: Value at 'importDataSource.s3Url' failed to satisfy constraint: Member must satisfy regular expression pattern: ^s3:\\/\\/([^\\/]+)\\/(.*?([^\\/]+)\\/?)$", got "1 validation error detected: Value 'not-a-valid-s3-url' at 'importDataSource.s3Url' failed to satisfy constraint: Member must satisfy regular expression pattern: ^s3:\\/\\/([^\\/]+)\\/(.*?([^\\/]+)\\/?)$".

7) sesv2.v1-template-token-continues-the-v2-list — botocore ClientError: An error occurred (NotImplemented) when calling the CreateTemplate operation: "operation CreateTemplate is scaffolded but not implemented in this emulator"; services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_template.rs lines 3-12 carry the SCAFFOLDED, NOT IMPLEMENTED marker.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:

1. sesv2.email-identity-lifecycle — call ListEmailIdentities with any AWS SDK/CLI (it sends GET /v2/email/identities); expect a 200 list, observe 501 NotImplemented.
2. sesv2.email-identity-verification-info-separates-a-missing-dns-zone-from-a-pending-one — CreateEmailIdentity for <name>.invalid and for <name>.example.com, then poll GetEmailIdentity on each; expect VerificationInfo with ErrorType HOST_NOT_FOUND and TYPE_NOT_FOUND respectively plus a live LastCheckedTimestamp, observe VerificationInfo absent on every poll until the check's deadline.
3. sesv2.email-identity-inherits-verified-parent-domain-dkim — with a domain identity verified to DkimAttributes.Status=SUCCESS, CreateEmailIdentity for an address under that domain, then GetEmailIdentity on the address; expect SigningEnabled=true, Status=SUCCESS and the parent's Tokens, observe SigningEnabled=false.
4. sesv2.list-management-options-unknown-list-rejected-contact-auto-created — SendEmail with ListManagementOptions.ContactListName naming a list that does not exist; expect 404 NotFoundException "List with name: <name> doesn't exist.", observe 501 NotImplemented.
5. sesv2.send-email-list-management-options-are-resolved-against-the-contact-list — SendEmail with a valid ListManagementOptions pair and an unverified FromEmailAddress; expect 400 MessageRejected "Email address is not verified. The following identities failed the check in region <REGION>: <sender>", observe 501 NotImplemented today and a region-less message once the stub is implemented.
6. sesv2.import-job-invalid-data-source-rejected — CreateImportJob with ImportDataSource.S3Url="not-a-valid-s3-url"; compare the BadRequestException message against the value-less AWS form.
7. sesv2.v1-template-token-continues-the-v2-list — SES v1 CreateTemplate, then v1 ListTemplates(MaxItems=1) and feed its NextToken to SESv2 ListEmailTemplates; expect the v2 page to continue the same list, observe 501 NotImplemented on CreateTemplate.
