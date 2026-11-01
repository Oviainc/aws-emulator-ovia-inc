Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-as2-certificate-and-profile-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-as2-certificate-and-profile-lifecycle transfer-certificate-blob-preserved
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeCertificate|2xx->2xx|$.Certificate.Tags
Issue-Key: transfer|DescribeCertificate|2xx->2xx|$.Certificate.Status
Issue-Commit: 6337d9239ecb247caeba8d19dc687e0a28fa9f5b

# Transfer DescribeCertificate reads diverge: imported certificate reports INACTIVE and empty Tags is omitted

> _Note: this issue was identified against the code base at commit `6337d9239ecb247caeba8d19dc687e0a28fa9f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Two net-new divergences share one fault surface: the DescribedCertificate projection the emulator serves for DescribeCertificate does not match the service's — the certificate's lifecycle Status is computed wrongly for a freshly imported certificate, and a present-but-empty Tags collection is suppressed instead of serialised as []. Both reads answer HTTP 200 on both sides, so nothing signals the divergence to a caller. TWO causes must be closed before submitting.

Cause 1 of 2 — DescribeCertificate omits Certificate.Tags when the certificate has no tags. Fixture transfer-as2-certificate-and-profile-lifecycle, step task#2 (the same cluster also covers task#5 of that fixture).
AWS: Smithy: DescribeCertificate -> DescribeCertificateResponse.Certificate (required) -> DescribedCertificate{..., Tags -> Tags, @length min 1 max 50, documented 'Key-value pairs that can be used to group and search for certificates'}. The @length bound governs a tag list a caller SUPPLIES; in the response the service renders the member for an untagged certificate as an empty array (recorded behavior only: the recorded body carries "Tags": []).
Emulator: the Tags member is absent from the body entirely (replay: '$.Certificate.Tags: missing (expected [], got None)').
Why: a client that reads Tags unconditionally — len(resp['Certificate']['Tags']), iterating it, or filtering certificates by tag — raises KeyError/NoneType on every untagged certificate instead of seeing zero tags, and the distinction between 'no tags' and 'field not returned by this API' is destroyed.
Fix: the DescribedCertificate serializer — render present-but-empty modelled collections as [] rather than suppressing them. This is the same empty-collection rule already filed for other shapes in the open issue 20261029T184519Z-transfer-reads-omit-empty-collection-members-and-the.md (DescribedAgreement.Tags, DescribedProfile.{CertificateIds,Tags}, ListTagsForResourceResponse.Tags, DescribedWorkflow.Tags); that issue does NOT name DescribedCertificate and this cluster was not reported as owned by it, so the certificate projection must be closed here.
Traces: transfer-as2-certificate-and-profile-lifecycle#task#2 (cluster also covers transfer-as2-certificate-and-profile-lifecycle#task#5).

Cause 2 of 2 — DescribeCertificate reports Status 'INACTIVE' for a certificate that the service reports as 'ACTIVE'. Fixture transfer-certificate-blob-preserved, step task#2.
AWS: Smithy: DescribedCertificate.Status -> CertificateStatusType (values ACTIVE, PENDING_ROTATION, INACTIVE), documented: "A certificate's status can be either ACTIVE or INACTIVE. You can set ActiveDate and InactiveDate in the UpdateCertificate call. If you set values for these parameters, those values are used to determine whether the certificate has a status of ACTIVE or INACTIVE. If you don't set values for ActiveDate and InactiveDate, we use the NotBefore and NotAfter date as specified on the X509 certificate to determine when a certificate is active and when it is inactive." So the requirement is: Status is derived from the effective validity window (ActiveDate/InactiveDate when supplied, otherwise the certificate's NotBeforeDate/NotAfterDate) evaluated against the current time — a certificate imported inside its validity window reads ACTIVE. The recording of this scenario reads back Status 'ACTIVE' immediately after the import.
Emulator: the read answers Status 'INACTIVE' (replay: '$.Certificate.Status: value_mismatch (expected ACTIVE, got INACTIVE)'; the same cluster also carries the Cause 1 omission '$.Certificate.Tags: missing (expected [], got None)'). Hypothesis: the projection stores a fixed or default status rather than evaluating the active/inactive window, or treats an unsupplied ActiveDate as 'not yet active' instead of falling back to NotBeforeDate.
Why: Status is what a caller checks to decide whether an imported AS2 signing/encryption certificate is usable before wiring it into CreateProfile.CertificateIds or an agreement. A certificate the service would serve as ACTIVE reading back INACTIVE makes callers conclude the import is unusable, makes 'wait until ACTIVE' automation never converge, and makes certificate-rotation logic rotate a certificate that is in fact live.
Fix: the DescribeCertificate handler / certificate state — persist the import's ActiveDate/InactiveDate (and the X509 NotBeforeDate/NotAfterDate fallback) and derive Status from that window at read time instead of emitting a constant. Note the open issue 20261029T184519Z-transfer-control-plane-writes-are-not-reflected-in.md covers imported certificates being invisible to ListCertificates; it does not cover the Status derivation in DescribeCertificate, and this cluster was not reported as owned by it.
Traces: transfer-certificate-blob-preserved#task#2.

General note: the concrete certificate ids, ARNs, fingerprints and dates in the recordings are per-run values, not requirements; the requirements are the presence of Tags as [] when empty and a Status derived from the certificate's effective validity window.

## Evidence
From replay of the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false). Expected = recorded AWS response, Actual = emulator. Both steps answered HTTP 200 on both sides (expected_status 200, actual_status 200), so only the body members diverge.

1. DescribeCertificate / transfer-as2-certificate-and-profile-lifecycle#task#2 (verdict: mismatch, cluster also affects transfer-as2-certificate-and-profile-lifecycle#task#5):
   detail: '$.Certificate.Tags: missing (expected [], got None)'
   divergence_key: service=transfer|DescribeCertificate|status=200->200|keys=$.Certificate.Tags
   stub_path: (empty), owned_by: (empty) — net-new, not covered by any open issue's replay key.

2. DescribeCertificate / transfer-certificate-blob-preserved#task#2 (verdict: mismatch):
   detail: '$.Certificate.Status: value_mismatch (expected ACTIVE, got INACTIVE)' and '$.Certificate.Tags: missing (expected [], got None)'
   divergence_key: service=transfer|DescribeCertificate|status=200->200|keys=$.Certificate.Status,$.Certificate.Tags
   stub_path: (empty), owned_by: (empty) — net-new.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DescribeCertificate takes CertificateId (@length 22, pattern ^cert-([0-9a-f]{17})$) and returns the required member Certificate -> DescribedCertificate. DescribedCertificate.Status targets CertificateStatusType (ACTIVE | PENDING_ROTATION | INACTIVE) and its documentation states the ActiveDate/InactiveDate-then-NotBefore/NotAfter derivation quoted above. DescribedCertificate.Tags targets Tags with @length min 1 max 50. The empty-array-rather-than-omitted rendering of Tags is recorded behavior only — the model does not state it.

## Steps to Reproduce
1. git clone the project repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Re-replay each bound step in its fixture's order so the preceding import runs first, and diff the JSON body against the recording:
   - transfer-as2-certificate-and-profile-lifecycle#task#2 — ImportCertificate with no Tags, then DescribeCertificate on the returned CertificateId; assert the response body contains Certificate.Tags == [] (present, empty), not an absent member. The same assertion must hold at task#5 of that fixture.
   - transfer-certificate-blob-preserved#task#2 — import the certificate as the fixture does, then DescribeCertificate on it; assert Certificate.Status == 'ACTIVE' (derived from the effective ActiveDate/InactiveDate, falling back to the X509 NotBeforeDate/NotAfterDate) and Certificate.Tags == [].
4. Both steps must report verdict 'pass' with HTTP 200 and no missing/value_mismatch keys under $.Certificate.
