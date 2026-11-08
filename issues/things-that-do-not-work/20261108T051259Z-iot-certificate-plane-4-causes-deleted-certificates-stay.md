Issue-Source: trace-sweep-divergence
Issue-Trace: iot-certificate-list-lifecycle
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-certificate-list-lifecycle iot-iot-certificate-transfer-lifecycle iot-register-thing-with-certificate iot-vpce-traj-rollback-order-refusals iot-certificate-transfer-cancel iot-certificate-policy-attachment-to-thing
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeCertificate|4xx->2xx|$.message
Issue-Key: iot|DescribeCertificate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeCertificate|4xx->2xx|status
Issue-Key: iot|DescribeCertificate|2xx->2xx|$.certificateDescription.transferData
Issue-Key: iot|DescribeCertificate|2xx->2xx|$.certificateDescription.customerVersion
Issue-Key: iot|DescribeCertificate|2xx->2xx|$.certificateDescription.status
Issue-Key: iot|TransferCertificate|4xx->4xx|$.message
Issue-Key: iot|TransferCertificate|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 89584cbdc793a0842735dd0ecc1a58bc9d0678bb

# iot certificate plane: 4 causes — deleted certificates stay readable, status updates never apply, transferData omitted, transfer validation reworded

> _Note: this issue was identified against the code base at commit `89584cbdc793a0842735dd0ecc1a58bc9d0678bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Four net-new divergences in the iot certificate plane: a deleted certificate remains readable, UpdateCertificate's status change is never applied to the stored certificate, DescribeCertificate drops the transferData projection, and TransferCertificate's constraint-violation message is reworded.

Cause 1 of 4: DescribeCertificate serves 200 for a certificate that has been deleted.
AWS: Smithy DescribeCertificate — GET /certificates/{certificateId}, success code 200, errors include ResourceNotFoundException (httpError 404). Once DeleteCertificate has removed a certificate, the registry no longer holds that certificateId and a read of it is refused. Recorded response: 404, header x-amzn-errortype "ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/", body message "CertificateId <certificateId> does not exist" — fixed text "CertificateId " ... " does not exist", with the certificateId taken verbatim from the request's URI label (the recorded run's 64-hex id is one instance of that variable, not part of the template).
Emulator: answers 200 with a certificate body, and sends no message and no x-amzn-errortype header (detail: "status: expected 404, got 200; $.message: missing; $headers.x-amzn-errortype: missing").
Why: deletion is unobservable. A client that deletes a certificate and then reads it back to confirm removal concludes the delete silently failed; cleanup loops never terminate, and any code that treats a successful read as "the principal is still usable" keeps operating on a credential the account no longer has. There is no 404 to key the ResourceNotFoundException path on at all.
Fix: DeleteCertificate's effect on the certificate store and the DescribeCertificate handler's lookup — a deleted certificateId must leave the store, and the read must raise ResourceNotFoundException with the 404 status, the iot-namespaced x-amzn-errortype header and AWS's "CertificateId <id> does not exist" message.
Traces: iot-certificate-list-lifecycle#teardown#9 (cluster also covers iot-certificate-list-lifecycle#teardown#10, iot-iot-certificate-transfer-lifecycle#teardown#9, iot-register-thing-with-certificate#teardown#11, iot-vpce-traj-rollback-order-refusals#task#16).

Cause 2 of 4: DescribeCertificate omits certificateDescription.transferData.
AWS: Smithy CertificateDescription member transferData targets the TransferData structure (members transferMessage, rejectReason, transferDate, acceptDate, rejectDate). AWS projects that structure on a described certificate even when no transfer has happened, with its member values unset. Recorded response: 200 with certificateDescription.transferData present as a structure whose five members are all unset.
Emulator: leaves the member out entirely (detail: "$.certificateDescription.transferData: missing").
Why: a client that reads certificateDescription.transferData to decide whether a certificate is mid-transfer, or that walks its acceptDate/rejectDate/rejectReason members, dereferences an absent structure instead of an empty one; in strongly-typed SDKs the member deserializes as null and the transfer-state branch is never reachable, so a pending or rejected transfer cannot be distinguished from a never-transferred certificate.
Fix: the DescribeCertificate serializer / its certificate read projection — emit the transferData structure for every certificate, with unset members where no transfer has occurred.
Traces: iot-certificate-list-lifecycle#task#4 (cluster also covers iot-certificate-transfer-cancel#task#4).

Cause 3 of 4: UpdateCertificate's newStatus is not applied and customerVersion is not advanced.
AWS: Smithy UpdateCertificate — PUT /certificates/{certificateId} with required @httpQuery newStatus (CertificateStatus enum), success code 200 with an empty body; it mutates the certificate's status. Smithy CertificateDescription members status (CertificateStatus enum) and customerVersion (integer, @range min 1) report that mutation, and a customer-initiated change advances customerVersion. Recorded behavior: after UpdateCertificate with newStatus INACTIVE, DescribeCertificate reports status "INACTIVE" and customerVersion 2.
Emulator: DescribeCertificate still reports status "ACTIVE" and customerVersion 1, and again omits transferData (detail: "$.certificateDescription.customerVersion: value_mismatch (expected 2, got 1); $.certificateDescription.status: value_mismatch (expected 'INACTIVE', got 'ACTIVE'); $.certificateDescription.transferData: missing"). UpdateCertificate itself answered success, so the mutation is accepted and discarded.
Why: deactivating a certificate is the primary way to revoke a device's access, and it is a silent no-op here. A client that deactivates a compromised certificate and verifies via DescribeCertificate sees ACTIVE and either loops or reports the revocation failed, while anything gating on status believes the credential is still live. The unchanged customerVersion also breaks optimistic-concurrency style checks.
Fix: the UpdateCertificate handler and the certificate state it writes — persist newStatus onto the stored certificate and advance its customerVersion, so the DescribeCertificate projection reflects the change.
Traces: iot-certificate-policy-attachment-to-thing#task#10.

Cause 4 of 4: TransferCertificate's constraint-violation message quotes the rejected value and renders the pattern anchored.
AWS: Smithy TransferCertificate input targetAwsAccount — @httpQuery targetAwsAccount, required, @length min 12 max 12, @pattern ^[0-9]+$. AWS's framework validation message for a member that trips two constraints is the fixed template "<n> validation errors detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>; Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — the variable parts are the error count, the member name from the request, the modelled bound, and the pattern as AWS renders it, unanchored ([0-9]+). AWS does NOT interpolate the rejected value into the sentence. Recorded response: 400 with exactly that un-valued, unanchored form.
Emulator: 400 with the rejected value spliced in and the regex printed with its anchors — "2 validation errors detected: Value '<rejected value>' at 'targetAwsAccount' failed to satisfy constraint: Member must have length less than or equal to 12; Value '<rejected value>' at 'targetAwsAccount' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]+$". Status and error count are right; the fixed text differs in two ways. (The recorded x-amzn-errortype for this step is not asserted, so the header class is not part of this cause.)
Why: the message is a requirement of the error class, and echoing caller input into it both diverges from AWS's wording and reflects unbounded request text back to the client. Assertions and log parsers keyed on AWS's "Value at '<member>' failed to satisfy constraint" form do not match, and the printed pattern differs from the one the model publishes.
Fix: the shared request-validation message builder used by the TransferCertificate handler's input validation — drop the "Value '<x>'" interpolation and render the pattern as the model's unanchored form. The open issue 20261107T022027Z-iot-refusal-envelopes-8-causes-wrong-x-amzn.md covers the same rewording for other iot operations but does not carry this step's replay key, so this cause is filed here.
Traces: iot-iot-certificate-transfer-lifecycle#task#3.

## Evidence
Cause 1 — fixture iot-certificate-list-lifecycle step teardown#9, DescribeCertificate after delete: expected 404 with $.message "CertificateId <id> does not exist" and $headers.x-amzn-errortype "ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/"; actual 200 with neither.
Cause 2 — fixture iot-certificate-list-lifecycle step task#4, DescribeCertificate: expected $.certificateDescription.transferData present as a structure with transferMessage/rejectReason/transferDate/acceptDate/rejectDate unset; actual member absent.
Cause 3 — fixture iot-certificate-policy-attachment-to-thing step task#10, DescribeCertificate after UpdateCertificate newStatus=INACTIVE: expected $.certificateDescription.status "INACTIVE" and $.certificateDescription.customerVersion 2; actual "ACTIVE" and 1 (plus transferData absent).
Cause 4 — fixture iot-iot-certificate-transfer-lifecycle step task#3, TransferCertificate with a non-numeric 31-character targetAwsAccount: both sides 400, but expected $.message "2 validation errors detected: Value at 'targetAwsAccount' failed to satisfy constraint: Member must have length less than or equal to 12; Value at 'targetAwsAccount' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+" while the emulator interpolated the rejected value and printed ^[0-9]+$.
Model citations: DescribeCertificate GET /certificates/{certificateId} code 200, errors incl. ResourceNotFoundException (404); CertificateDescription.transferData -> TransferData{transferMessage,rejectReason,transferDate,acceptDate,rejectDate}; CertificateDescription.status (CertificateStatus enum), .customerVersion (integer @range min 1); UpdateCertificate PUT /certificates/{certificateId} with required @httpQuery newStatus; TransferCertificate.targetAwsAccount @length 12..12 @pattern ^[0-9]+$.

## Steps to Reproduce
1. Replay fixture iot-certificate-list-lifecycle: at task#4 DescribeCertificate omits certificateDescription.transferData; at teardown#9, after the certificate is deleted, DescribeCertificate still answers 200 instead of 404 ResourceNotFoundException with "CertificateId <id> does not exist".
2. Replay fixture iot-certificate-policy-attachment-to-thing to step task#10: call UpdateCertificate with newStatus=INACTIVE, then DescribeCertificate — status is still ACTIVE and customerVersion still 1.
3. Replay fixture iot-iot-certificate-transfer-lifecycle step task#3: call TransferCertificate with a non-numeric targetAwsAccount longer than 12 characters and compare $.message to AWS's un-valued, unanchored two-error validation sentence.
