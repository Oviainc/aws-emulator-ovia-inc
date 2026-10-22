Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.request-certificate-default-ct-logging-enabled acm.certificate-transparency-logging-preference-deprecated-field-ignored acm.request-certificate-dns-validation-token-shape acm.request-certificate-domain-validation-options acm.request-certificate-pending-validation acm.request-certificate-dns-validation-resource-record acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated acm.dns-validation-cname-collapses-wildcard-onto-its-apex-and-is-unique-per-san acm.dns-validated-certificate-issues-under-a-delegated-zone acm.request-certificate-wildcard-position-rejected acm.op-contract-listTagsForCertificate acm.tag-resource-add-and-list acm.request-certificate-idempotency-token-charset acm.list-tags-for-resource-malformed-arn-rejected acm.request-certificate-too-many-tags-rejected acm.describe-certificate-not-found acm.tag-resource-nonexistent-resource-not-found
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.request-certificate-default-ct-logging-enabled|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: acm.certificate-transparency-logging-preference-deprecated-field-ignored|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Key: acm.request-certificate-dns-validation-token-shape|run#22|KeyError|assert opt["ValidationDomain"] == name, (
Issue-Key: acm.request-certificate-domain-validation-options|run#8|KeyError|assert entry["ValidationDomain"] == domain, entry
Issue-Key: acm.request-certificate-pending-validation|run#11|AssertionError|assert desc["KeyAlgorithm"] == "RSA-2048", desc.get("KeyAlgorithm")
Issue-Key: acm.request-certificate-dns-validation-resource-record|run#13|AssertionError|assert _CNAME_VALUE_RE.match(record["Value"]), (
Issue-Key: acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated|run#17|AssertionError|assert (len(target) > 3 and _TOKEN_RE.match(target[0]) is not None and target[1]
Issue-Key: acm.dns-validation-cname-collapses-wildcard-onto-its-apex-and-is-unique-per-san|run#15|AssertionError|assert records[wildcard] == records[apex], (
Issue-Key: acm.dns-validated-certificate-issues-under-a-delegated-zone|run#18|AssertionError|assert issued["Status"] == "ISSUED", (
Issue-Key: acm.request-certificate-wildcard-position-rejected|_error#15|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: acm.op-contract-listTagsForCertificate|run#6|KeyError|fresh = acm.list_tags_for_certificate(CertificateArn=arn)["Tags"]
Issue-Key: acm.tag-resource-add-and-list|run#7|KeyError|initial = acm.list_tags_for_certificate(CertificateArn=cert_arn)["Tags"]
Issue-Key: acm.request-certificate-idempotency-token-charset|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: acm.list-tags-for-resource-malformed-arn-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: acm|RequestCertificate|ValidationException
Issue-Key: acm.describe-certificate-not-found|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: acm.tag-resource-nonexistent-resource-not-found|run#8|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# ACM certificate representation, DNS validation and error-message contracts diverge (8 causes)

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificate

## Description
Eight distinct root causes in ACM's certificate projection, DNS-validation engine, request validation and error envelope. All are grounded in behavioral checks that pass on real AWS.

Cause 1 of 8: the DescribeCertificate projection omits Options and DomainValidationOptions[*].ValidationDomain and emits KeyAlgorithm in the request-side enum form.
AWS: Smithy CertificateDetail declares Options (CertificateOptions{CertificateTransparencyLoggingPreference, Export}) and DomainValidation.ValidationDomain; a public certificate is always CT-logged, so Options.CertificateTransparencyLoggingPreference is present and 'ENABLED' from the moment of request. Recorded behavior: DescribeCertificate returns KeyAlgorithm='RSA-2048' (hyphenated wire form) for a certificate requested with KeyAlgorithm RSA_2048, and each DomainValidationOptions entry carries ValidationDomain.
Emulator: $.Certificate.Options is never populated (two checks polled DescribeCertificate for 60s waiting for Options.CertificateTransparencyLoggingPreference and timed out), DomainValidationOptions entries have no ValidationDomain key (KeyError: 'ValidationDomain'), and KeyAlgorithm comes back 'RSA_2048'.
Why: a client reading cert.Options.CertificateTransparencyLoggingPreference gets None and concludes CT logging is off/unknown; email-validation tooling that derives the notification domain from ValidationDomain crashes; algorithm comparisons against the value AWS returns never match.
Fix: the DescribeCertificate response projection / CertificateDetail serializer in the ACM service crate, plus carrying CertificateOptions through RequestCertificate into certificate state. Note: open issue 20261022T122353Z-acm-describecertificate-omits-modeled-certificate-metadata-and-mis-forms records the same projection from the WIRE corpus; this cause records the behavioral checks that are gated on it.
Traces: acm.request-certificate-default-ct-logging-enabled, acm.certificate-transparency-logging-preference-deprecated-field-ignored, acm.request-certificate-dns-validation-token-shape, acm.request-certificate-domain-validation-options, acm.request-certificate-pending-validation.

Cause 2 of 8: the DNS validation CNAME value is not the ACM-shaped target.
AWS: Smithy DomainValidation.ResourceRecord{Name, Type=CNAME, Value} — recorded behavior: Value is '_<32 hex>.<per-validation label>.acm-validations.aws.' (a token label, then a label, then the acm-validations domain), and the check asserts the target decomposes as _<token>.<label>.acm-validations.<domain>.
Emulator: Value is '_<32 hex>.acm-validations.aws.' — the middle label is missing, so the target has one label too few.
Why: automation that parses the CNAME target (or publishes it and then verifies delegation structure) rejects the record, and any renewal flow keyed on the ACM validation sub-domain shape breaks.
Fix: the validation-record generator behind RequestCertificate / the certificate state that mints ResourceRecord values.
Traces: acm.request-certificate-dns-validation-resource-record, acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated.

Cause 3 of 8: a wildcard SAN does not share its apex's validation record.
AWS: recorded behavior — ACM strips the leading '*.' label when minting a validation record, so '*.example.com' and 'example.com' in the same request are handed one IDENTICAL CNAME (same Name and same Value), one record per distinct base domain.
Emulator: each entry gets its own independently generated record: Name '_d4446a3c....<domain>' / Value '_4050ea6d....acm-validations.aws' for the wildcard versus '_fe4ef5b4....<domain>' / '_a3b78a0a....acm-validations.aws' for the apex.
Why: a client that publishes one record per distinct base domain (what AWS requires) leaves one of the two entries unvalidated forever; a client that publishes both writes two conflicting CNAMEs at the same name on real AWS.
Fix: the validation-record generator — collapse the wildcard label onto the apex and deduplicate per base domain.
Traces: acm.dns-validation-cname-collapses-wildcard-onto-its-apex-and-is-unique-per-san.

Cause 4 of 8: a DNS-validated certificate never reaches ISSUED after its validation CNAME is published.
AWS: recorded behavior (noted in the check header) — with the validation CNAME published in a hosted zone the certificate moves to Status ISSUED with per-domain ValidationStatus SUCCESS within the first minute; Type AMAZON_ISSUED. Smithy CertificateStatus includes ISSUED and FailureReason exists only when Status is FAILED.
Emulator: the check (which, when ctx.is_real_aws is false, creates its own hosted zone in the emulated Route 53 and publishes the exact record DescribeCertificate handed it) observes Status 'FAILED' with FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED'.
Why: no DNS-validated certificate can ever be issued in the emulator, so every downstream flow that needs an ISSUED public certificate (listener/ custom-domain attachment, renewal, export) is unreachable; worse, the terminal FAILED state tells a client to abandon a request real AWS would have completed.
Fix: the DNS-validation state machine — observe the published record in the emulated Route 53 and drive the certificate to ISSUED/SUCCESS instead of FAILED.
Traces: acm.dns-validated-certificate-issues-under-a-delegated-zone.

Cause 5 of 8: RequestCertificate accepts a wildcard in an illegal position.
AWS: Smithy RequestCertificate.DomainName targets DomainNameString with pattern ^(\*\.)?(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$ — a '*' is admissible only as the whole leftmost label; anything else is refused (400, modeled InvalidParameterException/ValidationException class).
Emulator: the call SUCCEEDED with HTTP 200 and returned a CertificateArn ('arn:aws:acm:us-west-2:000000000000:certificate/d4be940a-...').
Why: this is overly permissive — the emulator mints a certificate for a name AWS would never accept, so a template or script with a malformed wildcard passes in test and fails in production.
Fix: request validation for RequestCertificate.DomainName/SubjectAlternativeNames (enforce the modeled pattern on every name, including the wildcard-position rule).
Traces: acm.request-certificate-wildcard-position-rejected.

Cause 6 of 8: ListTagsForCertificate omits the Tags member when the tag set is empty.
AWS: Smithy ListTagsForCertificate -> ListTagsForCertificateResponse{Tags: TagList}; recorded behavior — a freshly requested certificate answers with Tags present and empty ([]).
Emulator: HTTP 200 with no 'Tags' key at all (KeyError: 'Tags' in both checks on the first read of an untagged certificate).
Why: every client that does response['Tags'] (the documented shape) raises instead of seeing an empty list; tag reconciliation loops cannot distinguish "no tags" from "field absent".
Fix: the ListTagsForCertificate response serializer — emit the empty list rather than omitting the member.
Traces: acm.op-contract-listTagsForCertificate, acm.tag-resource-add-and-list.

Cause 7 of 8: constraint-violation ValidationException messages are built the wrong way.
AWS: recorded behavior, consistent across three members — the message is '1 validation error detected: Value of the input at '<member>' failed to satisfy constraint: Member must <constraint>', i.e. the offending value is NOT echoed and the regular expression is quoted WITHOUT the model's ^...$ anchors (e.g. "...regular expression pattern: \w+" for IdempotencyToken, whose Smithy pattern is ^\w+$, and "...pattern: arn:[\w+=/,.@-]+:acm:..." for resourceArn, whose Smithy pattern is ^arn:...$).
Emulator: echoes the request value and the anchored pattern: "Value 'bad-token-with-hyphens' at 'idempotencyToken' ... pattern: ^\w+$", "Value 'not-an-arn-at-allnot-an-arn-at-all' at 'resourceArn' ... pattern: ^arn:...$", "Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50". Status 400 and code ValidationException are correct in all three.
Why: the rule, not the recorded text, is the requirement — ACM's validation envelope never reflects the submitted value back (it can carry secrets such as an idempotency token) and quotes the pattern as the service does. A client or test that matches ACM's envelope cannot parse these, and value echoing leaks request content into logs.
Fix: the shared constraint-violation message builder for this service's ValidationException. Note: open issue 20261022T122353Z-acm-validationexception-messages-echo-the-input-value-and records the same rule from the WIRE corpus; this cause adds the behavioral checks.
Traces: acm.request-certificate-idempotency-token-charset, acm.list-tags-for-resource-malformed-arn-rejected, acm.request-certificate-too-many-tags-rejected.

Cause 8 of 8: the certificate-not-found message drops its trailing period.
AWS: recorded behavior — ResourceNotFoundException for an unknown certificate ARN carries exactly 'Could not find certificate <arn>.' (built from the request, terminated by a period).
Emulator: 400 ResourceNotFoundException (correct code) with 'Could not find certificate <arn>' — no terminating period.
Why: the message is produced by a known rule from the request, so the rule is the requirement; clients and fixtures that compare ACM's not-found text byte-for-byte diverge on every miss.
Fix: the not-found error construction shared by DescribeCertificate and the certificate tag operations. Note: open issue 20261022T122353Z-acm-certificate-not-found-message-drops-the-trailing records the same text from the WIRE corpus; this cause adds the behavioral checks.
Traces: acm.describe-certificate-not-found, acm.tag-resource-nonexistent-resource-not-found.

## Evidence
Cause 1: acm.request-certificate-default-ct-logging-enabled and acm.certificate-transparency-logging-preference-deprecated-field-ignored — 'Check timed out after 60.0s' while polling DescribeCertificate for d['Options'].get('CertificateTransparencyLoggingPreference'); acm.request-certificate-dns-validation-token-shape and acm.request-certificate-domain-validation-options — KeyError: 'ValidationDomain'; acm.request-certificate-pending-validation — assert desc['KeyAlgorithm'] == 'RSA-2048' failed with 'RSA_2048'.
Cause 2: acm.request-certificate-dns-validation-resource-record — AssertionError: ResourceRecord.Value='_6014dfb005daee2a98b5f462d4a3aa49.acm-validations.aws.' is not of the form _<32 hex>.<label>.acm-validations.<domain>.; acm.dns-validation-withholds-issuance-when-the-zone-is-not-publicly-delegated — AssertionError: the challenge target must be _<token>.<label>.acm-validations.<domain>; got '_d1c0327c8003e229e6f12035bad52578.acm-validations.aws.'.
Cause 3: acm.dns-validation-cname-collapses-wildcard-onto-its-apex-and-is-unique-per-san — AssertionError: ... must be handed an IDENTICAL CNAME; got ('_d4446a3ca911ca649fc3e01fd926dafc.<domain>', '_4050ea6d75967b01f5c7803dbc7b2dd4.acm-validations.aws') for the wildcard and ('_fe4ef5b414a54bf9af940a8da70ff0f3.<domain>', '_a3b78a0a140b2d52a6d071eada39ce89.acm-validations.aws') for the apex.
Cause 4: acm.dns-validated-certificate-issues-under-a-delegated-zone — AssertionError: with its validation CNAME publicly resolvable the certificate must reach ISSUED; got 'FAILED' (FailureReason 'ADDITIONAL_VERIFICATION_REQUIRED').
Cause 5: acm.request-certificate-wildcard-position-rejected — AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'CertificateArn': 'arn:aws:acm:us-west-2:000000000000:certificate/d4be940a-5d20-4474-b803-7245d79aeb3c', ResponseMetadata.HTTPStatusCode: 200}.
Cause 6: acm.op-contract-listTagsForCertificate and acm.tag-resource-add-and-list — KeyError: 'Tags' on acm.list_tags_for_certificate(CertificateArn=<fresh cert>)['Tags'] (expected []).
Cause 7: acm.request-certificate-idempotency-token-charset — expected "1 validation error detected: Value of the input at 'idempotencyToken' failed to satisfy constraint: Member must satisfy regular expression pattern: \\w+", got "... Value 'bad-token-with-hyphens' at 'idempotencyToken' ... pattern: ^\\w+$"; acm.list-tags-for-resource-malformed-arn-rejected — same divergence on 'resourceArn'; acm.request-certificate-too-many-tags-rejected — expected "... Value of the input at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", got "... Value '51 items' at 'tags' ...".
Cause 8: acm.describe-certificate-not-found and acm.tag-resource-nonexistent-resource-not-found — expected 'Could not find certificate arn:aws:acm:us-west-2:000000000000:certificate/00000000-0000-0000-0000-000000000000.', got the same string without the trailing '.'.
Smithy citations: CertificateDetail{Options, KeyAlgorithm, DomainValidationOptions}; CertificateOptions{CertificateTransparencyLoggingPreference, Export}; DomainValidation{ValidationDomain, ResourceRecord}; ResourceRecord{Name, Type=CNAME, Value}; RequestCertificate.DomainName pattern ^(\\*\\.)?...$; IdempotencyToken len 1..32 pattern ^\\w+$; Arn pattern ^arn:[\\w+=/,.@-]+:acm:...$; TagList len 1..50; ListTagsForCertificateResponse.Tags.

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: acm.request_certificate(DomainName=<d>, ValidationMethod='DNS'), then DescribeCertificate on the ARN — $.Certificate.Options is absent, DomainValidationOptions[*].ValidationDomain is absent, KeyAlgorithm is 'RSA_2048'.
3. Causes 2-3: request a certificate for '<d>' plus SAN '*.<d>' and read DomainValidationOptions[*].ResourceRecord — the Value has no middle label, and the wildcard and apex get different records.
4. Cause 4: create a hosted zone in the emulated Route 53, publish the CNAME DescribeCertificate returned, poll DescribeCertificate — Status becomes FAILED/ADDITIONAL_VERIFICATION_REQUIRED instead of ISSUED.
5. Cause 5: acm.request_certificate(DomainName='a.*.example.com') — returns 200 with an ARN.
6. Cause 6: acm.list_tags_for_certificate on an untagged certificate — the response has no 'Tags' member.
7. Cause 7: acm.request_certificate(..., IdempotencyToken='bad-token-with-hyphens') and acm.list_tags_for_resource(ResourceArn='not-an-arn') — compare Error.Message to ACM's envelope.
8. Cause 8: acm.describe_certificate on a non-existent certificate ARN — Error.Message lacks the trailing period.
9. Re-replay the 17 checks in trace_ids.
