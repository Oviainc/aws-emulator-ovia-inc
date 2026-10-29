Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.import-certificate-unsupported-key-size-rejected
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.import-certificate-unsupported-key-size-rejected|run#22|AssertionError|assert big_arn in summaries, (
Issue-Commit: ad2eaa5d43ab5cd0296a6d5c6ea30e6c9d7fc9f9

# ACM ListCertificates applies the default keyTypes filter to an explicitly-supplied empty Includes, hiding non-default key algorithms (1 cause)

> _Note: this issue was identified against the code base at commit `ad2eaa5d43ab5cd0296a6d5c6ea30e6c9d7fc9f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificates

## Description
Cause 1 of 1: ListCertificates treats a PRESENT `Includes` structure that carries no `keyTypes` member the same as an OMITTED `Includes`, and so applies the documented default key-algorithm filter instead of no key-algorithm filter. A certificate whose key algorithm is outside the default set (e.g. RSA-4096) is therefore invisible to a caller that explicitly opted out of key-type filtering by sending `Includes={}`.

AWS: Smithy `Filters$keyTypes` documents only the OMITTED-filter behavior — "Default filtering returns only RSA_1024 and RSA_2048 certificates that have at least one domain. To return other certificate types, provide the desired type signatures in a comma-separated list." The model says nothing about an `Includes` structure sent WITHOUT `keyTypes`, so for that case: recorded behavior only — the recording shows real ACM returning the RSA-4096 certificate in `CertificateSummaryList` for a `ListCertificates` call whose `Includes` is present and empty (HTTP 200), i.e. the narrow default `keyTypes` set is the fallback for an absent `Includes`, not for an `Includes` whose `keyTypes` is absent. The same recording shows ACM returning that certificate when `Includes.keyTypes` names the whole KeyAlgorithm enum, which the emulator already does correctly.

Emulator: `ListCertificates` with `Includes={}` answers HTTP 200 with a `CertificateSummaryList` that does NOT contain the imported RSA-4096 certificate's ARN, while the same store listed with `Includes={"keyTypes":[RSA_1024,RSA_2048,RSA_3072,RSA_4096,EC_prime256v1,EC_secp384r1,EC_secp521r1]}` does contain it. The check fails on `assert big_arn in summaries` — "the unfiltered listing cannot see the RSA-4096 control certificate". Every other assertion in the check passes, including the RSA-1536 refusal (`ValidationException` / "ACM does not support this key type."), so the divergence is confined to how the key-type filter's default is chosen.

Why: a client that sends an explicit empty `Includes` to opt out of filtering gets a listing that silently omits certificates whose key algorithm is not RSA_1024/RSA_2048. The response is a 200 with a short list, not an error, so the client concludes those certificates do not exist — it will re-import duplicates, skip renewal/rotation of a real certificate, or treat a successful import as having been dropped. It also makes any "the certificate is absent" assertion over this channel vacuous, because the channel cannot see the key algorithms it is being asked about.

Fix: the `ListCertificates` handler's filter predicate. The default key-algorithm set must be selected on whether the request supplied `Includes` AT ALL, not on whether `Includes.keyTypes` is populated: when `Includes` is present and `keyTypes` is absent, no key-algorithm filtering applies; when `Includes` is absent entirely, the documented default set applies. Nothing else in the operation (status filter, key-pair-origin default, sort, paging) changes.

Traces: acm.import-certificate-unsupported-key-size-rejected.

## Evidence
Check: acm.import-certificate-unsupported-key-size-rejected [mismatch]

Expected (recorded against AWS): after ImportCertificate of a self-signed RSA-4096 leaf, `ListCertificates(Includes={})` -> HTTP 200 and `CertificateSummaryList` contains that certificate's ARN (with `KeyAlgorithm="RSA-4096"`), because an explicitly supplied `Includes` with no `keyTypes` applies no key-algorithm filter.

Actual (emulator): `ListCertificates(Includes={})` -> HTTP 200, `CertificateSummaryList` does not contain the ARN. Replay failure:
  File "check_acm_import_certificate_unsupported_key_size_rejected.py", line 230, in run
    assert big_arn in summaries, (
  AssertionError: the unfiltered listing cannot see the RSA-4096 control certificate, so it could not have seen a leaked 1536-bit one either -- the absence check below would be vacuous

Contrast within the same run (both pass): `DescribeCertificate` reports `Status=ISSUED`, `KeyAlgorithm="RSA-4096"`, `SignatureAlgorithm="SHA256WITHRSA"`; `ListCertificates(Includes={"keyTypes":[<all KeyAlgorithm members>]})` DOES return the ARN with `KeyAlgorithm="RSA-4096"`. So the record and the projection are right; only the choice of default when `Includes` is present-but-empty is wrong.

Model citation: Smithy `Filters$keyTypes` — "Default filtering returns only RSA_1024 and RSA_2048 certificates that have at least one domain." `ListCertificatesRequest$Includes` -> `Filters`; `Filters.keyTypes` is optional. Handler location: services/service-com-amazonaws-acm-certificatemanager/src/ops/list_certificates.rs, `matches_filters` (the `excluded_by_key_type` computation around lines 321-325) passes `includes.and_then(|i| i.key_types.as_deref())` to `selects(...)`, whose `None` arm falls back to `crate::state::DEFAULT_LISTED_KEY_ALGORITHMS` — so a present-but-empty `Includes` is indistinguishable from an absent one.

No open issue covers ListCertificates filtering (checked: no issue matching 'listcertificates'; the other open ACM issues concern ACME, PCA, DNS validation and certificate representation).

## Steps to Reproduce
1. Boot the emulator and point an `acm` client at it.
2. Mint a self-signed RSA-4096 leaf (CN = a test domain, SAN = the same domain) and call ImportCertificate(Certificate=<pem>, PrivateKey=<pem>). Note the returned CertificateArn.
3. DescribeCertificate(CertificateArn=<arn>) -> confirms Status=ISSUED and KeyAlgorithm="RSA-4096" (passes today).
4. Call ListCertificates(Includes={"keyTypes": ["RSA_1024","RSA_2048","RSA_3072","RSA_4096","EC_prime256v1","EC_secp384r1","EC_secp521r1"]}) and page it -> the ARN IS present (passes today).
5. Call ListCertificates(Includes={}) (an explicitly supplied, empty Filters structure — not an omitted Includes) and page it -> AWS returns the ARN; the emulator's CertificateSummaryList omits it.

Equivalently: run the behavioral check acm.import-certificate-unsupported-key-size-rejected; it fails at the `big_arn in summaries` assertion after all earlier assertions pass.
