Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfoac-oac-update-enum-casing-mistakes
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfoac-oac-update-enum-casing-mistakes cloudfront-cfode-attach-rewrite-request-shape-mistakes cloudfront-cfoac-tenant-discovery-route-mistakes cloudfront-icc-caller-reference-retry-and-conflict cloudfront-cfode-distribution-lookup-identifier-mistakes cloudfront-icc-readback-identifier-and-paging-mistakes
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|UpdateOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListDistributionTenantsByCustomization|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListDistributions|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|ListInvalidations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261105T231732Z-cloudfront-refusals-carry-the-wrong-http-status-and.md
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFront validation and pagination-marker messages do not use AWS wording

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginAccessControl

## Description
Six net-new wire divergences where the emulator returns the right status and (mostly) the right error code but an error MESSAGE that is not AWS's. An error message is a requirement: callers log it, match on it, and surface it to operators, and in several cases below the emulator's wording asserts a different rule than AWS's. The open issue 20261029T212223Z-cloudfront-duplicate-caller-reference-409-messages-do-not.md covers CloudFront's 409 duplicate-caller-reference wording on other operations and other replay keys; it does not cover the steps below, nor does 20261102T102137Z-cloudfront-six-message-representation-divergences-illegalupdate-tenant-parameter.md.

Cause 1 of 6: UpdateOriginAccessControl's enum-constraint message lists the allowed values in the wrong order.
AWS: recorded behavior — the message's fixed text is "1 validation error detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [always-amz-auth, never, always, no-override]", where the value and member path come from the request and the bracketed set is fixed. Note AWS's runtime order differs from the model's declaration order (Smithy: OriginAccessControlSigningBehaviors values [never, always, no-override, always-amz-auth]), so AWS's order is the requirement, not the model's.
Emulator: emits the same message but with the set in the model's declaration order, [never, always, no-override, always-amz-auth]. 2 affected steps.
Why: callers and test suites assert on this message verbatim; a differently ordered value set fails an exact-match assertion and, for an operator reading the log, suggests a different enum definition than AWS publishes.
Fix: the shared enum-constraint message renderer used by the OriginAccessControl validation — it must emit AWS's fixed ordering rather than iterating the model's enum.
Traces: cloudfront-cfoac-oac-update-enum-casing-mistakes#task#2 (2 affected steps).

Cause 2 of 6: UpdateDistribution's function-association eventType message uses the wrong count form, the wrong constraint phrase and the wrong value set.
AWS: recorded behavior — fixed text "N validation errors detected: Value '<value>' at '<member path>' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]". Three things are fixed: the plural "validation errors detected" form CloudFront uses on this path, the phrase "Member must satisfy value set" (not "enum value set"), and the two-value set — Smithy: FunctionAssociation.EventType is documented "either viewer-request or viewer-response. You cannot use origin-facing event types (origin-request and origin-response) with a CloudFront function", even though the EventType enum itself has four values. The offending value and member path come from the request.
Emulator: emits "1 validation error detected: ... Member must satisfy enum value set: [viewer-request, viewer-response, origin-request, origin-response]".
Why: the emulator's message tells the caller that origin-request/origin-response ARE acceptable for a CloudFront function association, which is the opposite of the rule being enforced — so the caller retries with another rejected value.
Fix: the UpdateDistribution function-association validation must render CloudFront's "value set" wording restricted to the viewer-facing event types.
Traces: cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#9.

Cause 3 of 6: ListDistributionTenantsByCustomization's filter refusal carries a message about a different rule.
AWS: Smithy: ListDistributionTenantsByCustomization input has optional WebACLArn and CertificateArn, and binds InvalidArgument (httpError 400). Recorded refusal message, entirely fixed text: "Exactly one customization filter must be set."
Emulator: answers 400 with "You must specify either the CertificateArn parameter or WebACLArn parameter, but not both in the same request."
Why: the emulator's wording only covers the both-set case, so a caller that set NEITHER filter is given a message that does not describe its mistake. AWS's single sentence covers zero and two alike.
Fix: ListDistributionTenantsByCustomization's input validation message.
Traces: cloudfront-cfoac-tenant-discovery-route-mistakes#task#3.

Cause 4 of 6: CreateInvalidation's duplicate-caller-reference refusal is emulator-authored and leaks per-run values.
AWS: Smithy: CreateInvalidation binds InvalidArgument (400). Recorded refusal message, entirely fixed text with no variable parts: "Your request contains a caller reference that was used for a previous invalidation batch for the same distribution."
Emulator: answers 400 with "The CallerReference <reference> was already used for invalidation <invalidation-id> of this distribution with different paths" — a message AWS does not emit, interpolating the caller reference and a generated invalidation id.
Why: the message is not AWS's, so exact-match handling fails; worse, it embeds a generated invalidation id, which is a per-run value that can never be part of a stable contract.
Fix: CreateInvalidation's caller-reference conflict path must emit AWS's fixed message.
Traces: cloudfront-icc-caller-reference-retry-and-conflict#task#4.

Cause 5 of 6: ListDistributions' invalid-Marker refusal is emulator-authored and leaks the submitted marker.
AWS: Smithy: ListDistributions input Marker is an httpQuery string and the operation binds only InvalidArgument (400). Recorded refusal message, entirely fixed text: "Marker must be a valid Distribution Id".
Emulator: answers 400 with "Marker <marker> is not a distribution id returned by ListDistributions", interpolating the submitted marker.
Why: AWS states a format rule the caller can act on; the emulator states a membership rule that is both differently worded and misleading (a well-formed id from a prior page need not still be present). Clients matching AWS's message do not recognise the refusal.
Fix: ListDistributions' Marker validation message.
Traces: cloudfront-cfode-distribution-lookup-identifier-mistakes#task#10.

Cause 6 of 6: ListInvalidations' invalid-Marker refusal is emulator-authored and leaks the marker and distribution id.
AWS: Smithy: ListInvalidations input Marker is an httpQuery string; errors AccessDenied/InvalidArgument/NoSuchDistribution. Recorded refusal message, entirely fixed text: "Marker must be a valid Invalidation Id".
Emulator: answers 400 with "Marker <marker> is not the id of an invalidation of distribution <distribution-id>", interpolating both the marker and a generated distribution id.
Why: as Cause 5 — the same shared marker-validation path produces non-AWS wording across the list operations, and it embeds a generated distribution id that is a per-run value.
Fix: the shared CloudFront pagination-marker validation message used by the list handlers.
Traces: cloudfront-icc-readback-identifier-and-paging-mistakes#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 78.2%, 102 divergence clusters, all cloudfront). In every cause below the HTTP status matches and only the Message (and, in Cause 2, the count/constraint phrasing) diverges.

Cause 1 — cloudfront-cfoac-oac-update-enum-casing-mistakes#task#2 UpdateOriginAccessControl (400/400): $.ErrorResponse.Error.Message expected "1 validation error detected: Value 'Always' at 'originAccessControlConfig.signingBehavior' failed to satisfy constraint: Member must satisfy enum value set: [always-amz-auth, never, always, no-override]"; got the same text with set '[never, always, no-override, always-amz-auth]' (also #task#4).
Cause 2 — cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#9 UpdateDistribution (400/400): expected "1 validation errors detected: Value 'Viewer-Request' at 'distributionConfig.defaultCacheBehavior.functionAssociations.items.1.member.eventType' failed to satisfy constraint: Member must satisfy value set: [viewer-request, viewer-response]"; got "1 validation error detected: ... Member must satisfy enum value set: [viewer-request, viewer-response, origin-request, origin-response]".
Cause 3 — cloudfront-cfoac-tenant-discovery-route-mistakes#task#3 ListDistributionTenantsByCustomization (400/400): expected 'Exactly one customization filter must be set.'; got 'You must specify either the CertificateArn parameter or WebACLArn parameter, but not both in the same request.'
Cause 4 — cloudfront-icc-caller-reference-retry-and-conflict#task#4 CreateInvalidation (400/400): expected 'Your request contains a caller reference that was used for a previous invalidation batch for the same distribution.'; got 'The CallerReference sim2real-probe-icc-ref2-64910b98-r was already used for invalidation IGZNI4PND7H2S3 of this distribution with different paths'.
Cause 5 — cloudfront-cfode-distribution-lookup-identifier-mistakes#task#10 ListDistributions (400/400): expected 'Marker must be a valid Distribution Id'; got 'Marker tenant-distribution is not a distribution id returned by ListDistributions'.
Cause 6 — cloudfront-icc-readback-identifier-and-paging-mistakes#task#6 ListInvalidations (400/400): expected 'Marker must be a valid Invalidation Id'; got 'Marker not-an-invalidation is not the id of an invalidation of distribution E7IYCGPKTANSF3'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-cfoac-oac-update-enum-casing-mistakes and inspect step task#2 (representative).
4. Replay cloudfront-cfode-attach-rewrite-request-shape-mistakes#task#9, cloudfront-cfoac-tenant-discovery-route-mistakes#task#3, cloudfront-icc-caller-reference-retry-and-conflict#task#4, cloudfront-cfode-distribution-lookup-identifier-mistakes#task#10 and cloudfront-icc-readback-identifier-and-paging-mistakes#task#6.
5. Compare ErrorResponse/Error/Message against the recorded response in each — the fixed text must match AWS exactly, with only request/state-derived values (the offending value, member path, marker) substituted. All six causes must match before closing.
