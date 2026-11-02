Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.create-invalidation-for-tenant-batch-too-large-rejected cloudfront.list-domain-conflicts-invalid-argument-rejected cloudfront.update-anycast-ip-list-invalid-argument-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.create-invalidation-for-tenant-batch-too-large-rejected|run#9|AssertionError|assert code == "413", f"expected 413 (BatchTooLarge), got {code!r}"
Issue-Key: cloudfront.list-domain-conflicts-invalid-argument-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.update-anycast-ip-list-invalid-argument-rejected|run#10|AssertionError|assert code == "InvalidArgument", f"expected InvalidArgument, got {code!r} ({message!r})"
Issue-Commit: a949f0be934ac03c0270d0363880b6ddbcd0ac98

# cloudfront: three refusal divergences — oversized tenant invalidation batch answers TooManyInvalidationsInProgress instead of 413 BatchTooLarge, ListDomainConflicts refuses the domain string before the missing certificate, and an invalid IpAddressType enum answers unmodeled MalformedXML

> _Note: this issue was identified against the code base at commit `a949f0be934ac03c0270d0363880b6ddbcd0ac98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvalidationForDistributionTenant

## Description
Three distinct net-new root causes in the CloudFront control plane's refusal paths. All three are refusals where the emulator answers with the WRONG error shape/status or the WRONG message text, so a client cannot classify the failure the way it does against AWS. These are related in theme to the open issues 20261029T212223Z-cloudfront-refusal-codes-and-messages-diverge-from-aws.md and 20261102T050405Z-cloudfront-answers-404-entitynotfound-where-aws-refuses-the.md, but neither of those carries these replay keys and the suite did not mark these checks as owned by them.

=== Cause 1 of 3: CreateInvalidationForDistributionTenant answers TooManyInvalidationsInProgress for an oversized invalidation batch instead of BatchTooLarge (HTTP 413) ===
AWS: Smithy model (com.amazonaws.cloudfront, CreateInvalidationForDistributionTenant) binds POST /2020-05-31/distribution-tenant/{Id}/invalidation and models the error `BatchTooLarge` with @httpError 413 and documentation "Invalidation batch specified is too large." A batch whose InvalidationBatch.Paths quantity exceeds CloudFront's invalidation batch size limit is the BatchTooLarge condition: HTTP status 413, and botocore surfaces that condition's Error.Code as the literal HTTP status string "413" rather than the Smithy shape name (the check records this as confirmed against real AWS). The sibling modeled error `TooManyInvalidationsInProgress` (@httpError 400, "You have exceeded the maximum number of allowable InProgress invalidation batch requests, or invalidation objects.") is a DIFFERENT, concurrency-driven condition and is not what a size-limit violation produces. The fixed requirement here is the status (413) and the condition selected; the message is whatever AWS words for BatchTooLarge and is not asserted by the check.
Emulator: for a batch of 60,000 distinct paths submitted against a freshly created tenant with zero invalidations in flight, the emulator refuses with Error.Code 'TooManyInvalidationsInProgress' (HTTP 400). Replay detail: `AssertionError: expected 413 (BatchTooLarge), got 'TooManyInvalidationsInProgress'`. The emulator is counting the request against an in-progress/quantity quota instead of rejecting the batch for its size, and it does so on a tenant the same check just proved has 0 invalidations.
Why: a client distinguishes these two conditions to decide whether to retry. TooManyInvalidationsInProgress is transient — the documented remedy is to wait for in-flight invalidations to complete and retry the same batch — while BatchTooLarge is permanent and the remedy is to split the batch. A client relying on AWS's behavior will retry an oversized batch forever, and will never take the split-the-batch path. The wrong HTTP status (400 vs 413) also breaks generic payload-too-large handling in front of the SDK.
Fix: the size/limit validation of the InvalidationBatch in the CreateInvalidationForDistributionTenant handler's validation step — the batch-size check must run before (and independently of) the in-progress-invalidation quota check, and must raise BatchTooLarge with its modeled 413 status.
Traces: cloudfront.create-invalidation-for-tenant-batch-too-large-rejected

=== Cause 2 of 3: ListDomainConflicts refuses the Domain string before checking the validation resource's certificate, so the InvalidArgument message is the wrong one ===
AWS: Smithy model, ListDomainConflicts (POST /2020-05-31/domain-conflicts) takes required Domain and required DomainControlValidationResource and models InvalidArgument (@httpError 400, "An argument is invalid."). Recorded behavior: AWS evaluates the named DomainControlValidationResource FIRST; when that resource holds no CA-chain-valid certificate and no managed certificate request, AWS refuses with 400 InvalidArgument and the message "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" — for ANY value of Domain, whether the domain is syntactically malformed, merely over CloudFront's label-length limit, or perfectly well formed. That message is entirely fixed text: it names no request value and has no variable parts.
Emulator: the error code is right (InvalidArgument) but the message is not. For a DistributionId validation resource with no certificate the emulator answers 'The request contains an invalid domain name.' Replay detail: `AssertionError: expected message 'There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request', got 'The request contains an invalid domain name.'` The emulator validates the Domain string's syntax first and never inspects the validation resource for a certificate, so it reports the wrong reason for the refusal and would also report the wrong reason (or none) for a well-formed domain against the same certificate-less resource.
Why: the message is the only thing distinguishing "your domain string is malformed" from "the resource you pointed at has no certificate" — both are 400 InvalidArgument. A client or operator reading the emulator's message will go fix the domain name, which is not the problem, and a client that matches on the certificate wording will not match at all. The ordering divergence also means a caller passing a valid domain gets a different outcome than AWS gives.
Fix: the ListDomainConflicts handler's validation ordering — resolve and check DomainControlValidationResource for a covering certificate / managed certificate request before validating the Domain string, and emit AWS's certificate wording for that condition.
Traces: cloudfront.list-domain-conflicts-invalid-argument-rejected

=== Cause 3 of 3: UpdateAnycastIpList refuses an out-of-range IpAddressType enum with an unmodeled MalformedXML and a generic constraint message instead of InvalidArgument ===
AWS: Smithy model, UpdateAnycastIpList (PUT /2020-05-31/anycast-ip-list/{Id}) has member IpAddressType targeting the enum com.amazonaws.cloudfront#IpAddressType with values [ipv4, ipv6, dualstack], and models exactly these errors: AccessDenied (403), EntityNotFound (404), InvalidArgument (400), InvalidIfMatchVersion (400), PreconditionFailed (412), UnsupportedOperation (400). `MalformedXML` is NOT a modeled error of this operation. A value outside the enum's value set is an invalid argument and is refused with 400 InvalidArgument; the refusal message is AWS's own wording for that condition (the check does not assert the message text, noting the condition is not reachable on a live pool account because every pool account sits at the documented zero default quota for Anycast static IP lists, Service Quotas code L-6A19EDFD).
Emulator: answers Error.Code 'MalformedXML' with the message "1 validation error detected: Value 'ipv7-bogus' at 'ipAddressType' failed to satisfy constraint: Member must satisfy enum value set: [ipv4, ipv6, dualstack]". Replay detail: `AssertionError: expected InvalidArgument, got 'MalformedXML' ("1 validation error detected: Value 'ipv7-bogus' at 'ipAddressType' failed to satisfy constraint: Member must satisfy enum value set: [ipv4, ipv6, dualstack]")`. In that message the fixed text is the emulator's own generic constraint-violation template ("N validation error(s) detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [...]") — a template AWS uses for JSON-protocol services, not for CloudFront's REST-XML control plane — and the variable parts are the submitted value ('ipv7-bogus'), the lowerCamel member name ('ipAddressType' rather than the wire name IpAddressType) and the enum's value list. The code itself, MalformedXML, is not in this operation's modeled error set at all, so an SDK cannot deserialize it into a modeled exception class and it falls through to generic client-error handling.
Also observed on the same check, not the failing assertion: CreateAnycastIpList succeeded against the emulator, where the check records that real pool accounts are refused first with EntityLimitExceeded (whose documented message directs the caller to the L-6A19EDFD quota page) or UnsupportedOperation. That is account/quota state rather than emulator state, so it is reported here as context only and is not one of the three causes to close.
Why: a client that catches CloudFront's InvalidArgument to report a bad configuration value sees an unmodeled MalformedXML instead and treats the request as a wire/serialization fault rather than an argument it supplied — exactly the wrong diagnosis, and one that defeats retry/validation branching keyed on the modeled error.
Fix: the enum/shape validation layer behind the UpdateAnycastIpList handler — out-of-enum member values on a REST-XML CloudFront operation must be mapped to that operation's modeled InvalidArgument (400) with AWS's wording, not to the JSON-style generic constraint template under a MalformedXML code.
Traces: cloudfront.update-anycast-ip-list-invalid-argument-rejected

## Evidence
Cause 1 — cloudfront.create-invalidation-for-tenant-batch-too-large-rejected [mismatch]
  Expected (AWS): CreateInvalidationForDistributionTenant with 60,000 paths -> HTTP 413, BatchTooLarge (botocore Error.Code "413"), no invalidation created.
  Actual (emulator): Error.Code 'TooManyInvalidationsInProgress' (HTTP 400) on a tenant the check had just verified holds 0 invalidations.
  Replay: File "check_cloudfront_create_invalidation_for_tenant_batch_too_large_rejected.py", line 41, in run
          assert code == "413", f"expected 413 (BatchTooLarge), got {code!r}"
          AssertionError: expected 413 (BatchTooLarge), got 'TooManyInvalidationsInProgress'
  Model: CreateInvalidationForDistributionTenant errors include BatchTooLarge @httpError 413 and TooManyInvalidationsInProgress @httpError 400 (distinct conditions).

Cause 2 — cloudfront.list-domain-conflicts-invalid-argument-rejected [mismatch]
  Expected (AWS): 400 InvalidArgument, Message exactly "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" for both a malformed Domain ("not a valid domain !!") and an over-long but well-formed Domain (250-char label + ".example.com"), because the certificate-less DomainControlValidationResource is checked first.
  Actual (emulator): 400 InvalidArgument with Message 'The request contains an invalid domain name.'
  Replay: File "check_cloudfront_list_domain_conflicts_invalid_argument_rejected.py", line 46, in run
          assert message == expected, f"expected message {expected!r}, got {message!r}"
          AssertionError: expected message 'There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request', got 'The request contains an invalid domain name.'

Cause 3 — cloudfront.update-anycast-ip-list-invalid-argument-rejected [mismatch]
  Expected (AWS): UpdateAnycastIpList(IpAddressType="ipv7-bogus") -> 400 InvalidArgument (a modeled error of the operation); IpAddressType and ETag unchanged.
  Actual (emulator): Error.Code 'MalformedXML' — not a modeled error of UpdateAnycastIpList — with "1 validation error detected: Value 'ipv7-bogus' at 'ipAddressType' failed to satisfy constraint: Member must satisfy enum value set: [ipv4, ipv6, dualstack]".
  Replay: File "check_cloudfront_update_anycast_ip_list_invalid_argument_rejected.py", line 68, in run
          assert code == "InvalidArgument", f"expected InvalidArgument, got {code!r} ({message!r})"
          AssertionError: expected InvalidArgument, got 'MalformedXML' ("...Member must satisfy enum value set: [ipv4, ipv6, dualstack]")
  Model: UpdateAnycastIpList errors = AccessDenied(403), EntityNotFound(404), InvalidArgument(400), InvalidIfMatchVersion(400), PreconditionFailed(412), UnsupportedOperation(400). MalformedXML absent. IpAddressType enum values = [ipv4, ipv6, dualstack].

## Steps to Reproduce
Cause 1:
1. CreateDistribution for a multi-tenant distribution, then CreateDistributionTenant (disabled) under it.
2. ListInvalidationsForDistributionTenant(Id=tenant) and confirm Quantity == 0.
3. CreateInvalidationForDistributionTenant(Id=tenant, InvalidationBatch={CallerReference: <unique>, Paths: {Quantity: 60000, Items: [60000 distinct ~60-char paths]}}).
4. Observe the refusal: AWS gives HTTP 413 / BatchTooLarge (botocore Error.Code "413"); the emulator gives HTTP 400 / TooManyInvalidationsInProgress.

Cause 2:
1. CreateDistribution (minimal, Enabled=false) and keep its Id.
2. ListDomainConflicts(Domain="not a valid domain !!", DomainControlValidationResource={"DistributionId": <that id>}).
3. Repeat with Domain=("a"*250) + ".example.com".
4. Both calls must be 400 InvalidArgument with the "There must be at least one certificate in the distribution tenant or distribution, or there must be a managed certificate request" message; the emulator returns 'The request contains an invalid domain name.' instead.

Cause 3:
1. CreateAnycastIpList(Name=<unique>, IpCount=3) and capture Id + ETag.
2. UpdateAnycastIpList(Id=<id>, IfMatch=<etag>, IpAddressType="ipv7-bogus").
3. Expect 400 InvalidArgument (a modeled error of the operation); the emulator returns MalformedXML with a generic JSON-style constraint message.
4. GetAnycastIpList(Id=<id>) and confirm IpAddressType and ETag are unchanged by the rejected update.

Replay command: run the behavioral suite and re-run the three check ids in trace_ids.
