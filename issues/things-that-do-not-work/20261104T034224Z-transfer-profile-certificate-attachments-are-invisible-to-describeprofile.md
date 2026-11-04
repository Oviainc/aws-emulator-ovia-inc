Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-as2-certificate-and-profile-lifecycle
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-as2-certificate-and-profile-lifecycle
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeProfile|2xx->2xx|$.Profile.CertificateIds[*]
Issue-Commit: b8482007ab5cef3cdc98ee3694cf4b4d8c9dc6a3

# Transfer profile certificate attachments are invisible to DescribeProfile: CertificateIds comes back empty after a certificate is attached

> _Note: this issue was identified against the code base at commit `b8482007ab5cef3cdc98ee3694cf4b4d8c9dc6a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeProfile

## Description
ONE net-new root cause, found by replaying the wire corpus simparity-wire-aws-v1: after an AS2 certificate is imported and attached to a profile, a read of that profile reports no attached certificates.

Related open issue that does NOT cover this cause (named so a fixer can see the overlap): '20261029T184519Z-transfer-control-plane-writes-are-not-reflected-in.md' covers the same class of fault for agreement status, certificate records and workflows and owns those replay keys; it does not name the profile<->certificate association or DescribeProfile.CertificateIds, which is why this step is still diverging and is filed here. '20261101T164813Z-transfer-describecertificate-reads-diverge-imported-certificate-reports-inactive.md' covers DescribeCertificate's own fields, not the profile read.

Cause 1 of 1 - DescribeProfile reports an empty CertificateIds for a profile that has a certificate attached.
AWS: Smithy: DescribeProfile(ProfileId required, @length 19..19, @pattern ^p-([0-9a-f]{17})$) -> DescribedProfile, whose CertificateIds member is documented as 'An array of identifiers for the imported certificates. You use this identifier for working with profiles and partner profiles.' (list of CertificateId, @length 22..22, @pattern ^cert-([0-9a-f]{17})$). CreateProfile.CertificateIds and UpdateProfile.CertificateIds are the two writes that set that array; UpdateProfile is documented as 'Updates some of the parameters for an existing profile'. So the requirement is read-after-write: once a certificate id has been attached to a profile, every subsequent DescribeProfile of that profile lists that id in CertificateIds, and the list keeps the ids the last write established. Recorded behavior: HTTP 200 with Profile.CertificateIds containing the attached certificate's id (the recording's literal 'cert-00000000000000001' is just that run's minted id, not part of the requirement).
Emulator: HTTP 200 and an otherwise well-formed Profile, but CertificateIds is EMPTY - replay reports $.Profile.CertificateIds[0] missing (expected the attached certificate id, got None). The read itself is faithful to state: services/service-com-amazonaws-transfer-transferservice/src/ops/describe_profile.rs renders certificate_ids straight from the stored profile, and CreateProfile stores input.certificate_ids (src/ops/create_profile.rs). Hypothesis: the attachment never reaches state because the update write path is an unimplemented scaffold - services/service-com-amazonaws-transfer-transferservice/src/ops/update_profile.rs still carries the 'SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() returns Scaffolded::not_implemented("UpdateProfile"), so the profile's certificate_ids stays at the value CreateProfile left and the read correctly renders an empty list of a state that was never updated.
Why: a client that attaches a partner/local certificate and then reads the profile back to confirm the binding (the normal AS2 setup verification, and what IaC drift detection does) sees a profile with no certificates, so it either re-attaches forever or concludes the certificate import failed. Anything that drives signing/encryption off the profile's CertificateIds finds nothing to use, and an absent-vs-empty distinction a caller relies on to decide 'this profile still has certificates attached' is answered wrongly.
Fix: the profile WRITE path and the profile state it owns - the certificate-attachment update for a profile must land in the stored profile's certificate_ids (UpdateProfile's handler / crate::state::Profile), so the existing DescribeProfile read renders it. DescribeProfile's serializer is already correct and should not be changed.
Traces: transfer-as2-certificate-and-profile-lifecycle#task#11.

## Evidence
Replay of the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout: 309 steps, 264 passed, 38 mismatched, score 85.4%; emulator_up=true.

Fixture transfer-as2-certificate-and-profile-lifecycle, step task#11 (DescribeProfile), verdict mismatch, status 200 -> 200:
  $.Profile.CertificateIds[0]: missing (expected the attached certificate id - 'cert-00000000000000001' in the recording - got None)
Every other member of the described profile matched, and the status matched; only the certificate association is absent.

Source confirmation in this checkout:
- services/service-com-amazonaws-transfer-transferservice/src/ops/describe_profile.rs:162-176 builds DescribedProfile with certificate_ids: Some(stored.certificate_ids.clone()), i.e. the read mirrors state and is not the defect.
- services/service-com-amazonaws-transfer-transferservice/src/ops/create_profile.rs:191-199 stores input.certificate_ids.unwrap_or_default() on create.
- services/service-com-amazonaws-transfer-transferservice/src/ops/update_profile.rs:1-12 and 149-167 are still the scaffold: the 'SCAFFOLDED, NOT IMPLEMENTED' marker is present and handle() returns crate::scaffolded::Scaffolded::not_implemented("UpdateProfile"), so no post-create attachment can reach state.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DescribedProfile.CertificateIds -> CertificateIds (list of CertificateId, @length 22..22, @pattern ^cert-([0-9a-f]{17})$); UpdateProfile input is {ProfileId (required), CertificateIds} -> {ProfileId}, errors InvalidRequestException (400), ResourceNotFoundException (404), ThrottlingException (429), InternalServiceError/ServiceUnavailableException (503).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 against it.
3. Re-replay fixture transfer-as2-certificate-and-profile-lifecycle and inspect step task#11: ImportCertificate, create the AS2 profile, attach the imported certificate to the profile, then DescribeProfile on that ProfileId.
4. Compare $.Profile.CertificateIds against the recorded response: it must list the attached certificate's id; the emulator returns an empty array.
