Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rtbfabric.associate-and-disassociate-certificate-lifecycle rtbfabric.get-link-routing-rule-not-found rtbfabric.get-requester-gateway-not-found rtbfabric.list-requester-gateways-invalid-max-results-rejected rtbfabric.list-tags-for-resource-nonexistent-resource-not-found
Issue-Shape-Id: com.amazonaws.rtbfabric#RTBFabric
Issue-Key: rtbfabric.associate-and-disassociate-certificate-lifecycle|run#10|AssertionError|assert code == "ValidationException", (
Issue-Key: rtbfabric.get-link-routing-rule-not-found|run#6|AssertionError|assert error["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {error['Code']}"
Issue-Key: rtbfabric.get-requester-gateway-not-found|run#10|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Key: rtbfabric.list-requester-gateways-invalid-max-results-rejected|run#7|AssertionError|assert over["Message"] == over_message, f"expected {over_message!r}, got {over['Message']!r}"
Issue-Key: rtbfabric.list-tags-for-resource-nonexistent-resource-not-found|run#6|AssertionError|assert error["Code"] == "InternalServerErrorException", (
Issue-Commit: 04a5c1cce1738afed3911d32859c10309860f34e

# rtbfabric: 5 net-new causes — AssociateCertificate/GetLinkRoutingRule/ListTagsForResource refusals answer 501 NotImplemented, and the not-found and @range ValidationException messages are emulator-authored

> _Note: this issue was identified against the code base at commit `04a5c1cce1738afed3911d32859c10309860f34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateCertificate, GetLinkRoutingRule, GetRequesterGateway, ListRequesterGateways, ListTagsForResource

## Description
Five distinct root causes in the rtbfabric emulator, all found by the behavioral suite (2/10 checks passed). Three operations still answer the scaffold's 501 `NotImplemented` on refusal paths real AWS answers with a modeled 4xx/5xx, and two implemented paths serve emulator-authored error messages where AWS serves fixed wording. Each cause must be closed before submitting.

NOTE on scope overlap: separate stub issues are filed from the checkout for UNIMPLEMENTED `ListRequesterGateways`, `RejectLink` and `ListTagsForResource` (the checks rtbfabric.create-requester-gateway-invalid-vpc-pattern-rejected, rtbfabric.link-reject-terminal-state-blocks-accept-rejected and rtbfabric.tag-and-untag-resource-round-trip). Causes 1, 2 and 5 below are NOT covered by those: causes 1 and 2 name operations no filed issue names, and cause 5 records the specific refusal SEMANTICS the ListTagsForResource stub issue does not state.

=== Cause 1 of 5: AssociateCertificate answers 501 NotImplemented instead of refusing an INTERNAL gateway with 400 ValidationException ===
AWS: Smithy (services/service-com-amazonaws-rtbfabric-rtbfabric/smithy-model/com-amazonaws-rtbfabric-rtbfabric.json) models AssociateCertificate as POST /responder-gateway/{gatewayId}/certificate with modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). Certificate association is only supported on an EXTERNAL responder gateway; recorded behavior (real AWS, us-west-2) for an ACTIVE INTERNAL responder gateway with a well-formed ACM certificate ARN is HTTP 400, code `ValidationException`, message `Certificate association is only supported for EXTERNAL gatewayType` — that message is entirely fixed text, with no values taken from the request or state.
Emulator: the call is refused with code `NotImplemented` (HTTP 501) from the scaffold stub; the gateway's gatewayType is never consulted.
Why: a client that provisions TLS on a responder gateway cannot distinguish "wrong gateway type" from "service unavailable": 501 is not a modeled error for this operation, so SDK error handling for ValidationException never runs and the gatewayType precondition is invisible. The same check's next assertion (CreateResponderGateway for an EXTERNAL gateway whose managed-endpoint domain is not allowlisted must answer 400 ValidationException, message `Domain name '<domainName>' for managed endpoint not allowlisted.` — fixed text plus the requested domainName from the request) was never reached and is unverified; it must hold once cause 1 is fixed.
Fix: the AssociateCertificate handler (services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/associate_certificate.rs, handle() at line 169-187 returns Scaffolded::not_implemented) — implement the operation and its gatewayType precondition.
Traces: rtbfabric.associate-and-disassociate-certificate-lifecycle.

=== Cause 2 of 5: GetLinkRoutingRule answers 501 NotImplemented instead of 404 ResourceNotFoundException ===
AWS: Smithy models GetLinkRoutingRule as GET /responder-gateway/{gatewayId}/link/{linkId}/routing-rule/{ruleId}, errors ResourceNotFoundException(404), ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). For a format-valid but nonexistent gatewayId/linkId/ruleId triple (ids matching ^rtb-gw-[a-z0-9-]{1,25}$ / ^link-[a-z0-9-]{1,25}$ / ^rule-[a-z0-9-]{1,25}$ with plain lowercase-hex suffixes, so pattern validation passes and the existence check is reached), recorded behavior is HTTP 404, code `ResourceNotFoundException`, message `The requested resource was not found.` — fixed text, no variable parts.
Emulator: code `NotImplemented` (HTTP 501).
Why: the operation is unreachable; a client probing for a routing rule gets an unmodeled 501 instead of the not-found it must branch on.
Fix: the GetLinkRoutingRule handler (services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/get_link_routing_rule.rs) — implement lookup against stored links/routing rules and the modeled not-found refusal.
Traces: rtbfabric.get-link-routing-rule-not-found.

=== Cause 3 of 5: not-found refusals carry an emulator-authored message instead of AWS's fixed wording ===
AWS: rtbfabric's ResourceNotFoundException message for a well-formed but nonexistent gatewayId is the fixed string `The requested resource was not found.` (recorded behavior; the model documents the shape only as "The request could not be completed because the resource does not exist"). The message has NO variable parts — AWS does not echo the requested id.
Emulator: GetRequesterGateway returns ResourceNotFoundException whose message is composed from the request: `no requester gateway with id <gatewayId> exists` (observed with a nonexistent rtb-gw- id). The status and code are right; the text is emulator-authored. See services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/get_requester_gateway.rs lines 178-188, which format!()s the id into the message. The same pattern should be audited on every rtbfabric not-found path (requester/responder gateways, links, routing rules), since they share this construction.
Why: clients and test harnesses that assert on the error message (and log scrapers that key on it) see wording real AWS never emits, and the emulator leaks the requested id into a message AWS keeps constant.
Fix: the not-found refusal construction in the rtbfabric operation handlers / shared error helpers — emit the fixed message text.
Traces: rtbfabric.get-requester-gateway-not-found.

=== Cause 4 of 5: @range ValidationException message echoes the offending value where AWS omits it ===
AWS: Smithy ListRequesterGateways.maxResults is an Integer httpQuery param with @range min 1, max 10, default 10; the only client error modeled is ValidationException(400). Recorded behavior for maxResults=11 is HTTP 400, code `ValidationException`, message `1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 10`; for maxResults=0 the lower-bound form is `1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1`. Fixed text: `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>` / `... greater than or equal to <min>`; the variable parts are the member name from the request binding and the bound from the model — the SUBMITTED VALUE is NOT part of this message for rtbfabric.
Emulator: `1 validation error detected: Value '11' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 10` — it inserts `Value '<submitted>' at` where AWS writes `Value at`.
Why: a client asserting on the rejection text (or a fixture recorded against AWS) mismatches on every out-of-range numeric parameter, and the emulator echoes a caller-supplied value AWS deliberately does not.
Fix: the constraint-validation message renderer that produces the "1 validation error detected" envelope for @range violations on this service (the pre-handler validation layer, not the ListRequesterGateways handler — the refusal fires before the stubbed handler runs, which is why it is a separate cause from the ListRequesterGateways stub issue).
Traces: rtbfabric.list-requester-gateways-invalid-max-results-rejected.

=== Cause 5 of 5: ListTagsForResource on a nonexistent gateway ARN answers 501 NotImplemented instead of 500 InternalServerErrorException ===
AWS: Smithy models ListTagsForResource as GET /tags/{resourceArn} with resourceArn pattern ^arn:aws:rtbfabric:[a-zA-Z0-9_-]+:[0-9]{12}:gateway/[a-zA-Z0-9-]+(/link/...)?$ and errors ResourceNotFoundException(404), ValidationException(400), AccessDeniedException(403), ThrottlingException(429), InternalServerException(500). The model/doc say not-found, but recorded behavior only (verified twice on real AWS, retries exhausted, stable) for a syntactically valid gateway ARN whose gatewayId does not exist is HTTP 5xx with code `InternalServerErrorException` and message `Internal server error.` — fixed text, no variable parts. An emulator answering ResourceNotFoundException here would also NOT match AWS.
Emulator: code `NotImplemented` (HTTP 501).
Why: the tagging read path is unreachable, and when it is implemented the obvious doc-driven choice (404 ResourceNotFoundException) would still diverge — so the required answer is recorded here explicitly.
Fix: the ListTagsForResource handler (services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/list_tags_for_resource.rs) — implement the operation and this unknown-ARN refusal. The separately filed stub issue for this operation does not state the refusal class/message; close both against this behavior.
Traces: rtbfabric.list-tags-for-resource-nonexistent-resource-not-found.

## Evidence
Behavioral suite: rtbfabric 2/10 (20%), 8 failed.

Cause 1 — rtbfabric.associate-and-disassociate-certificate-lifecycle [mismatch]:
  AssertionError: expected ValidationException associating a certificate on an INTERNAL gateway, got NotImplemented
  expected: code ValidationException / HTTP 400 / message 'Certificate association is only supported for EXTERNAL gatewayType'
  actual:   code NotImplemented (HTTP 501, scaffold stub)

Cause 2 — rtbfabric.get-link-routing-rule-not-found [mismatch]:
  AssertionError: expected ResourceNotFoundException, got NotImplemented
  expected: code ResourceNotFoundException / HTTP 404 / message 'The requested resource was not found.'
  actual:   code NotImplemented (HTTP 501)

Cause 3 — rtbfabric.get-requester-gateway-not-found [mismatch]:
  AssertionError: expected 'The requested resource was not found.', got 'no requester gateway with id rtb-gw-<hex> exists'
  (the recorded run shows one generated id; the requirement is that the message is the fixed text with no id interpolated at all)
  source: src/ops/get_requester_gateway.rs:181-184 format!("no requester gateway with id {} exists", input.gateway_id)

Cause 4 — rtbfabric.list-requester-gateways-invalid-max-results-rejected [mismatch]:
  AssertionError: expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 10",
            got "1 validation error detected: Value '11' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 10"
  (code and status agree: ValidationException / 400; only the message form diverges. Lower bound, maxResults=0, must likewise read 'Value at ... greater than or equal to 1')

Cause 5 — rtbfabric.list-tags-for-resource-nonexistent-resource-not-found [mismatch]:
  AssertionError: expected InternalServerErrorException, got NotImplemented
  expected: code InternalServerErrorException / message 'Internal server error.'
  actual:   code NotImplemented (HTTP 501)

## Steps to Reproduce
Boot the emulator and run the rtbfabric behavioral checks (botocore client for service 'rtbfabric' pointed at the emulator endpoint):

1. Cause 1: create a VPC/subnets/SGs, create an INTERNAL responder gateway and wait for status ACTIVE, import an ACM certificate, then call AssociateCertificate(gatewayId=<that gateway>, acmCertificateArn=<cert arn>, clientToken=<uuid>). Expect ClientError 400 ValidationException 'Certificate association is only supported for EXTERNAL gatewayType'; the emulator answers 501 NotImplemented. (Then: CreateResponderGateway with gatewayType=EXTERNAL and a managedEndpointConfiguration domain that is not allowlisted must answer 400 ValidationException "Domain name '<domain>' for managed endpoint not allowlisted." and must create no gateway — ListResponderGateways count unchanged.)
2. Cause 2: call GetLinkRoutingRule(gatewayId='rtb-gw-'+18 hex chars, linkId='link-'+hex, ruleId='rule-'+hex) for ids never created. Expect 404 ResourceNotFoundException 'The requested resource was not found.'; the emulator answers 501 NotImplemented.
3. Cause 3: call GetRequesterGateway(gatewayId='rtb-gw-'+18 hex chars) for an id never created. Expect 404 ResourceNotFoundException with message exactly 'The requested resource was not found.'; the emulator returns 'no requester gateway with id <id> exists'.
4. Cause 4: call ListRequesterGateways(maxResults=11). Expect 400 ValidationException "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 10". Repeat with maxResults=0 on a client with parameter validation disabled (botocore short-circuits the lower bound locally) and expect the 'greater than or equal to 1' form. The emulator inserts "Value '<n>' at".
5. Cause 5: call ListTagsForResource(resourceArn='arn:aws:rtbfabric:<region>:<account>:gateway/rtb-gw-'+hex) for a gateway that does not exist. Expect code InternalServerErrorException with message 'Internal server error.'; the emulator answers 501 NotImplemented.
