Issue-Source: trace-sweep-divergence
Issue-Trace: route53-create-hosted-zone-duplicate-caller-reference-error
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-create-hosted-zone-duplicate-caller-reference-error route53-create-hosted-zone-invalid-domain-error route53-error-not-found-and-invalid-vpc route53-hosted-zone-create-verify-delete route53-hosted-zone-operations-on-nonexistent-id-error route53-hosted-zone-accelerated-recovery-toggle route53-vpc-association-authorization-lifecycle route53-public-hosted-zone-record-lifecycle route53-test-dns-answer-with-record-lifecycle route53-dnssec-status-guardrails
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|CreateHostedZone|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|CreateHostedZone|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: route53|DeleteHostedZone|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|DeleteHostedZone|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: route53|GetHostedZone|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|GetHostedZone|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: route53|ChangeResourceRecordSets|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# Route 53 error responses omit RequestId and return non-AWS error message forms

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHostedZone

## Description
Every Route 53 error body the emulator serializes is missing the RequestId element, and several of its messages are built to a different rule than the one AWS uses for the same refusal. HTTP statuses and error codes match throughout, so this is error-body fidelity, not classification. FIVE DISTINCT CAUSES are bundled here; all five must be closed.

Cause 1 of 5: ErrorResponse omits RequestId on every error path.
AWS: the Route 53 REST-XML error envelope is <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>...</RequestId></ErrorResponse>; the recordings assert RequestId's presence (shape only, '<shaped-not-asserted>', or an opaque recorded id such as 60519c0d-10fb-451b-ad2d-d1a80b633dd8 whose VALUE is environment-specific and not a requirement).
Emulator: '$.ErrorResponse.RequestId: missing ... got None' on every error cluster observed - CreateHostedZone 409 HostedZoneAlreadyExists, CreateHostedZone 400 InvalidDomainName, DeleteHostedZone 404 NoSuchHostedZone and GetHostedZone 404 NoSuchHostedZone.
Why: clients and SDK middleware read ErrorResponse/RequestId to populate the request id used for retry correlation and support escalation; with the element absent every Route 53 failure surfaces with no request id.
Fix: the service's REST-XML error serializer must always emit a RequestId element (any per-request opaque id) in the ErrorResponse envelope.
Traces: route53-create-hosted-zone-duplicate-caller-reference-error#task#2, route53-create-hosted-zone-invalid-domain-error#task#1, route53-error-not-found-and-invalid-vpc#task#3, route53-hosted-zone-accelerated-recovery-toggle#teardown#7. (The invalid-domain step also shows a message difference - expected 'not_a_valid_domain_!! is reserved by AWS!' vs the emulator's top-level-domain wording - but no documented rule produces that recorded text, so only the missing RequestId is claimed as a defect there.)

Cause 2 of 5: NoSuchHostedZone message is not AWS's request-derived form.
AWS: Smithy com.amazonaws.route53#NoSuchHostedZone (client error, HTTP 404, member message) is raised naming the id from the request; the recordings consistently show the message built as 'No hosted zone found with ID: <Id>' for GetHostedZone, DeleteHostedZone and the other zone-addressed operations - a request-derived message, so the FORM is the requirement, not any one id.
Emulator: 404 with the correct NoSuchHostedZone code but message 'no hosted zone exists with the ID <Id>' (the model's prose description reused verbatim, lowercased, with no colon), e.g. 'no hosted zone exists with the ID Z00000000000000000000'.
Why: tests and tooling that pattern-match the documented 'No hosted zone found with ID:' prefix to distinguish a missing zone from a malformed id do not match the emulator's text.
Fix: build the NoSuchHostedZone message in the shared zone-lookup helper to AWS's form 'No hosted zone found with ID: <requested id>'.
Traces: route53-error-not-found-and-invalid-vpc#task#3 (DeleteHostedZone, cluster of 3), route53-hosted-zone-accelerated-recovery-toggle#teardown#7 (GetHostedZone, cluster of 4), route53-public-hosted-zone-record-lifecycle#task#11 (cluster of 3), route53-dnssec-status-guardrails#task#8 (DeleteHostedZone 404; that step's expected message is unasserted, so it is listed for replay coverage of the same wording path). Handlers construct it at services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_hosted_zone.rs:153-157 and the equivalent lookups in the other zone ops.

Cause 3 of 5: HostedZoneAlreadyExists message leaks the generated zone id and the caller reference.
AWS: Smithy HostedZoneAlreadyExists (client error, HTTP 409) is documented as 'The hosted zone you're trying to create already exists. Amazon Route 53 returns this error when a hosted zone has already been created with the specified CallerReference' - and the recording shows the fixed message 'A hosted zone has already been created with the specified caller reference.', which contains no id and no echo of the caller reference.
Emulator: 409 with the right code but a message that embeds the caller reference and the existing zone's generated id ('a hosted zone has already been created with CallerReference <cr>: /hostedzone/<generated Z id>').
Why: the message is not the documented fixed text, and it exposes a server-generated zone id that differs on every run, so a client cannot compare or log it meaningfully.
Fix: emit the documented fixed message from the CreateHostedZone duplicate-caller-reference path; the zone id belongs in state, not in the refusal text.
Traces: route53-create-hosted-zone-duplicate-caller-reference-error#task#2.

Cause 4 of 5: HostedZoneNotEmpty message is not AWS's documented text.
AWS: Smithy HostedZoneNotEmpty (client error, HTTP 400). The recorded refusal is the fixed AWS text 'The specified hosted zone contains resource record sets other than the default SOA and NS records and so cannot be deleted. Delete all non-default records and try again.' - a constant that names no zone id and tells the caller what to do next.
Emulator: 400 with the right code but 'the hosted zone <Z id> contains resource records that are not the default SOA and NS records and so cannot be deleted' - the model's prose description, zone-id-interpolated, with the remediation sentence dropped.
Why: clients matching the documented sentence (and users reading it) lose the remediation instruction, and the id interpolation makes the text non-constant across runs.
Fix: emit the documented constant message from the DeleteHostedZone non-empty guard.
Traces: route53-public-hosted-zone-record-lifecycle#task#12.

Cause 5 of 5: InvalidChangeBatch for a duplicate record set is rendered as a flat internal message instead of AWS's per-change message list.
AWS: Smithy InvalidChangeBatch (client error, HTTP 400) carries messages -> ErrorMessages (a LIST, one entry per bad change) in addition to message. The recording shows the list form '[Tried to create resource record set [name=\'<name>\', type=\'<type>\'] but it already exists]' - bracketed list, each entry naming the record with name=/type= keys derived from the request.
Emulator: 400 with the right code but a single flat message 'Tried to create resource record set [Action=CREATE, Name=<name>, Type=<type>, SetIdentifier=null] but it already exists' - no list wrapper, and internal fields (Action, SetIdentifier=null) exposed with different key names.
Why: clients that read InvalidChangeBatch.messages to report which changes in a batch failed get nothing, and the per-change rendering does not match AWS's documented name=/type= form.
Fix: populate the modeled messages list in the ChangeResourceRecordSets validation path, one request-derived entry per rejected change, and render each in AWS's name=/type= form rather than dumping the internal change struct.
Traces: route53-public-hosted-zone-record-lifecycle#task#10.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0). Statuses matched in every cluster below; only the body diverged.

Cause 1 - CreateHostedZone 409->409: '$.ErrorResponse.RequestId: missing (expected \'<shaped-not-asserted>\', got None)'; CreateHostedZone 400->400: '$.ErrorResponse.RequestId: missing (expected \'60519c0d-10fb-451b-ad2d-d1a80b633dd8\', got None)'; DeleteHostedZone 404->404 and GetHostedZone 404->404: same RequestId omission.
Cause 2 - GetHostedZone/DeleteHostedZone 404->404: '$.ErrorResponse.Error.Message: value_mismatch (expected \'No hosted zone found with ID: Z00000000000000000000\', got \'no hosted zone exists with the ID Z00000000000000000000\')' and the same for Z31DI1RQF2TXNFR and Z00000000000000NOTREAL (clusters of 3, 4 and 3 steps).
Cause 3 - CreateHostedZone 409->409: expected 'A hosted zone has already been created with the specified caller reference.', got 'a hosted zone has already been created with CallerReference sim2real-probe-route53-dupzone-432ff155-cr: /hostedzone/Z2J9E1UPLCRGJ2G'.
Cause 4 - DeleteHostedZone 400->400: expected 'The specified hosted zone contains resource record sets other than the default SOA and NS records and so cannot be deleted. Delete all non-default records and try again.', got 'the hosted zone Z74GTXY8GF1YCEM contains resource records that are not the default SOA and NS records and so cannot be deleted'.
Cause 5 - ChangeResourceRecordSets 400->400: expected "[Tried to create resource record set [name=\'...\', type=\'A\'] but it already exists]", got 'Tried to create resource record set [Action=CREATE, Name=..., Type=A, SetIdentifier=null] but it already exists'.

Model evidence: smithy_query(shape=NoSuchHostedZone,HostedZoneAlreadyExists,HostedZoneNotEmpty,InvalidChangeBatch) on services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json - NoSuchHostedZone 404, HostedZoneAlreadyExists 409 (doc names the CallerReference rule), HostedZoneNotEmpty 400, InvalidChangeBatch 400 with members messages (ErrorMessages list) + message.

## Steps to Reproduce
1. Replay route53-create-hosted-zone-duplicate-caller-reference-error#task#2: create a zone, then CreateHostedZone again with the same CallerReference - the 409 body has no ErrorResponse/RequestId and the message embeds the caller reference and generated zone id.
2. Replay route53-error-not-found-and-invalid-vpc#task#3 and route53-hosted-zone-accelerated-recovery-toggle#teardown#7: GET/DELETE /2013-04-01/hostedzone/<nonexistent id> - the 404 message uses 'no hosted zone exists with the ID <id>' and carries no RequestId.
3. Replay route53-create-hosted-zone-invalid-domain-error#task#1 for the RequestId omission on a 400 InvalidDomainName.
4. Replay route53-public-hosted-zone-record-lifecycle#task#12: create a record set in a zone, then DeleteHostedZone - inspect the 400 HostedZoneNotEmpty message.
5. Replay route53-public-hosted-zone-record-lifecycle#task#10: CREATE a record set that already exists - inspect the 400 InvalidChangeBatch body for the messages list and the name=/type= rendering.
