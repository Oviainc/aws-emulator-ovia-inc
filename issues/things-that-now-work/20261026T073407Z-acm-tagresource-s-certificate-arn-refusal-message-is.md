Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.tag-resource-generic-visible-via-list-tags-for-certificate
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|TagResource|ValidationException
Issue-Commit: 4b6ee1390c91103628cf7a83eb97a10f4c7a2602

# ACM TagResource's certificate-ARN refusal message is reworded, not AWS's fixed text

> _Note: this issue was identified against the code base at commit `4b6ee1390c91103628cf7a83eb97a10f4c7a2602`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause in the ACM emulator.

Cause 1 of 1: the generic TagResource API correctly refuses a certificate ARN with ValidationException, but emits its own prose instead of ACM's message text.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) models TagResource(ResourceArn required, @length 20..2048, @pattern ^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$; Tags required, @length 1..50) -> Unit, errors [ResourceNotFoundException (client, httpError 400), ServiceQuotaExceededException (client, 400), ValidationException (client, 400, awsQueryError ValidationError)]. The operation's own documentation states the rule: "Use this action for all ACM resource types except the certificate resource type. For certificate resources, use AddTagsToCertificate instead." AWS enforces that by refusing a certificate ARN with HTTP 400 and Error.Code 'ValidationException'. The recorded message is a three-sentence template whose FIXED text is "TagResource only supports non-certificate resource types. Resource " / " is a certificate resource. Use AddTagsToCertificate API instead." and whose single VARIABLE part is the ResourceArn copied verbatim from the request (the account id, region and certificate uuid in any one recording are request/environment values that merely fill that slot, never part of the requirement).

Emulator: the status (400) and the code ('ValidationException') are correct, and the refusal is real — the tag is not applied, ListTagsForCertificate does not show it, and the positive control (TagResource against an acme-endpoint ARN, then ListTagsForResource) is served correctly. Only Error.Message diverges: the emulator answers 'arn:aws:acm:<region>:<account>:certificate/<uuid> names a certificate, which this operation does not apply to; use AddTagsToCertificate instead' — the ARN is moved to the front, the sentence "TagResource only supports non-certificate resource types." is dropped entirely, "is a certificate resource" becomes "names a certificate", "Use AddTagsToCertificate API instead." loses the word "API" and the terminating period, and the whole string is re-punctuated with a semicolon. No part of AWS's fixed text survives.

Why: an HTTP status, an error code and an error message are all requirements of a refusal. A client (or an SDK/CLI wrapper, or a test asserting on ACM's envelope) that recognises this specific misuse of the generic tagging API by its message — to retry the call through AddTagsToCertificate, or to surface ACM's own guidance to a user — matches nothing against the emulator and reports an unclassified ValidationException, while the same code passes against real ACM. The message is fully derivable from the request, so the template is the contract and this is a byte-level divergence for every certificate ARN passed to TagResource, not a per-run value difference.

Fix: the ValidationException construction in the ACM TagResource handler's ARN resource-type gate (services/service-com-amazonaws-acm-certificatemanager/src/ops/, the TagResource operation handler and the shared ARN-resolution/validation path the tag-writing operations dispatch through) — emit AWS's fixed text with the request's ResourceArn interpolated into the middle slot. This is the same family of defect as, but not covered by, the open issues 20261025T052312Z-acm-acme-endpoint-contracts-diverge-deleteacmeendpoint-is-served.md (UntagResource's ResourceNotFoundException message drops the ARN's resource-type token) and 20261025T172549Z-acm-certificate-representation-and-state-gating-diverge-in.md (certificate representation and state-gating messages); neither carries this check's replay key or mentions TagResource's certificate-ARN gate.

Traces: acm.tag-resource-generic-visible-via-list-tags-for-certificate.

## Evidence
Cause 1 — acm.tag-resource-generic-visible-via-list-tags-for-certificate [mismatch]: the check first proves TagResource works at all (create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', CertificateAuthority={'PublicCertificateAuthority': {}}, Contact='NOT_REQUIRED'), tag_resource(ResourceArn=<acme-endpoint arn>, Tags=[{Key:<control>,Value:'control'}]), list_tags_for_resource shows the control tag — this leg PASSES against the emulator). It then imports a self-signed RSA-2048 certificate and calls tag_resource(ResourceArn=<certificate arn>, Tags=[{Key:<key>,Value:'via-generic'}]). Error.Code is 'ValidationException' as expected, and the subsequent list_tags_for_certificate correctly does not contain the key, so only the message text is wrong.

AssertionError (check line 60): expected 'TagResource only supports non-certificate resource types. Resource arn:aws:acm:us-west-2:000000000000:certificate/1a2eae44-aa94-47aa-8f35-b3288548ee0b is a certificate resource. Use AddTagsToCertificate API instead.', got 'arn:aws:acm:us-west-2:000000000000:certificate/1a2eae44-aa94-47aa-8f35-b3288548ee0b names a certificate, which this operation does not apply to; use AddTagsToCertificate instead'.

Fixed text required: 'TagResource only supports non-certificate resource types. Resource ' + <ResourceArn from the request> + ' is a certificate resource. Use AddTagsToCertificate API instead.' Variable part: the ResourceArn only — the us-west-2 region, the 000000000000 account and the 1a2eae44-… certificate id above are that one run's request values filling the slot, not requirements. Smithy: TagResource errors [ResourceNotFoundException 400, ServiceQuotaExceededException 400, ValidationException 400 / awsQueryError ValidationError]; TagResource documentation: 'Use this action for all ACM resource types except the certificate resource type. For certificate resources, use AddTagsToCertificate instead.'

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Positive control (must keep working): acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', CertificateAuthority={'PublicCertificateAuthority': {}}, Contact='NOT_REQUIRED'); acm.tag_resource(ResourceArn=<AcmeEndpointArn>, Tags=[{'Key':'k','Value':'control'}]); acm.list_tags_for_resource(ResourceArn=<AcmeEndpointArn>) shows the tag.
3. Mint a self-signed RSA-2048 X.509 pair and acm.import_certificate(Certificate=<pem>, PrivateKey=<key pem>) to get a certificate ARN.
4. Call acm.tag_resource(ResourceArn=<certificate arn>, Tags=[{'Key':'k','Value':'via-generic'}]). Observe HTTP 400 with Error.Code 'ValidationException' (correct) but Error.Message '<arn> names a certificate, which this operation does not apply to; use AddTagsToCertificate instead' instead of AWS's 'TagResource only supports non-certificate resource types. Resource <arn> is a certificate resource. Use AddTagsToCertificate API instead.'
5. Confirm acm.list_tags_for_certificate(CertificateArn=<arn>) does NOT contain the key (the refusal itself is correct; only the message must change).
6. Re-replay the check listed in trace_ids.
