Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-domain-name-duplicate-bad-request apigatewayv2.create-routing-rule-missing-condition-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.create-domain-name-duplicate-bad-request|run#12|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}: {message!r}"
Issue-Key: apigatewayv2.create-routing-rule-missing-condition-rejected|run#7|AssertionError|assert message == (
Issue-Commit: e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29

# apigatewayv2: 2 net-new refusal-contract divergences — CreateDomainName duplicate answers ConflictException instead of BadRequestException, and CreateRoutingRule's empty-condition BadRequestException message is reworded (2 causes)

> _Note: this issue was identified against the code base at commit `e5a3e8eb74dcb2b43ac4e38f0bb7bcf8e9a5cd29`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainName

## Description
Two net-new root causes in the apigatewayv2 control-plane refusal contracts. Both are refusals that DO fire but carry the wrong classification or the wrong wording, so a client branching on Error.Code / Error.Message takes the wrong branch. The open issue 20261027T053233Z-apigatewayv2-9-net-new-root-causes-api-mapping.md touches the api-mapping/routing-rule refusal surface but does NOT carry either of these checks' replay keys, so neither cause is covered by it.

=== Cause 1 of 2: CreateDomainName on an already-existing domain name is classified ConflictException (409) instead of BadRequestException (400), with a reworded message ===
AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json) models CreateDomainName as POST /v2/domainnames with success code 201 and errors AccessDeniedException(403), BadRequestException(400), ConflictException(409), NotFoundException(404), TooManyRequestsException(429) — so BOTH candidate codes are modeled and the model alone does not pick one. Recorded behavior decides it: real AWS answers this collision with `BadRequestException` (HTTP 400) and the message `The domain name you provided already exists.` That message is entirely FIXED text — it contains no value taken from the request or from emulator state (notably it does NOT interpolate the submitted DomainName). The rejected create must also leave the pre-existing domain name untouched (GetDomainName still reports the original DomainNameArn), and an unrelated second domain name must still be creatable.
Emulator: answers `ConflictException` and the message `The domain name <DomainName> already exists.` — i.e. both the error code (and therefore the HTTP status, 409 rather than 400) and the message template diverge. The emulator's template interpolates the requested domain name where AWS's wording has the fixed phrase "the domain name you provided".
Why: a client that distinguishes a retryable/ignorable 409 conflict from a 400 input rejection mis-handles a duplicate custom-domain create. CDK/Terraform/CloudFormation providers commonly treat 409 ConflictException on a create as "already exists, adopt it" and 400 BadRequestException as "your request is wrong, fail the deploy" — so the emulator silently flips an idempotency decision. Any client string-matching the documented message also fails, because the emulator substitutes a request value into text AWS keeps constant.
Fix: the CreateDomainName handler's duplicate-name validation in the apigatewayv2 service crate — it must raise BadRequestException (400) with the fixed message `The domain name you provided already exists.` instead of ConflictException, and must not interpolate the submitted DomainName into the message.
Traces: apigatewayv2.create-domain-name-duplicate-bad-request.

=== Cause 2 of 2: CreateRoutingRule with a condition entry carrying no match clause is refused with the right code but the wrong message ===
AWS: Smithy models CreateRoutingRule as POST /v2/domainnames/{DomainName}/routingrules (success 201) with required Conditions (__listOfRoutingRuleCondition), required Actions and required Priority (range 1..1000000), and errors BadRequestException(400), ConflictException(409), NotFoundException(404), TooManyRequestsException(429). The model does not state the per-condition rule, so: recorded behavior only — real AWS refuses a Conditions entry that specifies neither matchBasePaths nor matchHeaders with `BadRequestException` (400) and the message `Invalid conditions for RoutingRule. Each condition can only be either matchBasePaths or matchHeaders.` That message is entirely FIXED text: it names the member alternatives in their wire (lowerCamel) spelling and carries NO index, no request value and no state value. No rule is created (ListRoutingRules must not report the submitted Priority), and a routing rule with a real condition on the same domain name is still accepted.
Emulator: answers BadRequestException (so the code and status are right) but with the message `Conditions[0] must specify exactly one of: MatchBasePaths, MatchHeaders.` — a different fixed phrasing, the SDK-shape (UpperCamel) member spellings instead of the wire spellings, and an interpolated list index (`[0]`) that AWS's message does not have.
Why: the status and code are usable, but any client, test suite or operator runbook that matches on AWS's documented wording sees an unrecognised failure; and the index the emulator invents implies a per-entry diagnostic contract AWS does not offer, so a client built against the emulator will parse an index out of real AWS responses and find none.
Fix: the CreateRoutingRule handler's condition validation in the apigatewayv2 service crate — emit the fixed AWS message verbatim, with no index and with the wire member names matchBasePaths/matchHeaders.
Traces: apigatewayv2.create-routing-rule-missing-condition-rejected.

## Evidence
Cause 1 — expected: Error.Code `BadRequestException` (HTTP 400) and Error.Message `The domain name you provided already exists.`
Actual (apigatewayv2.create-domain-name-duplicate-bad-request): `AssertionError: expected BadRequestException, got 'ConflictException': 'The domain name sim2real-behav-dup89c7f1c36794.example.com already exists.'` raised at check line 48 (`assert code == "BadRequestException"`). The domain label `sim2real-behav-dup89c7f1c36794.example.com` is a per-run generated name and is NOT part of the requirement — the requirement is that AWS's message does not contain that value at all. The second assertion in the check (`expected_message = "The domain name you provided already exists."`) is the message requirement.

Cause 2 — expected: Error.Code `BadRequestException` with Error.Message exactly `Invalid conditions for RoutingRule. Each condition can only be either matchBasePaths or matchHeaders.` for CreateRoutingRule(DomainName=<domain>, Priority=100, Conditions=[{}], Actions=[{InvokeApi:{ApiId:<api>,Stage:'dev'}}]).
Actual (apigatewayv2.create-routing-rule-missing-condition-rejected): `AssertionError: BadRequestException message 'Conditions[0] must specify exactly one of: MatchBasePaths, MatchHeaders.'` raised at check line 34. The code assertion on the preceding line passed, so only the message diverges.

## Steps to Reproduce
Cause 1:
1. Boot the emulator from this checkout.
2. Import a self-signed certificate into ACM for a domain name <d> (openssl req -x509 -newkey rsa:2048 -subj /CN=<d>).
3. apigatewayv2 CreateDomainName(DomainName=<d>, DomainNameConfigurations=[{CertificateArn:<arn1>, EndpointType:'REGIONAL', SecurityPolicy:'TLS_1_2'}]) — succeeds; note DomainNameArn.
4. Import a SECOND certificate for the same <d> and call CreateDomainName(DomainName=<d>, DomainNameConfigurations=[{CertificateArn:<arn2>, ...}]) so only the name collision is exercised.
5. Observe ConflictException / 'The domain name <d> already exists.' where AWS answers BadRequestException / 'The domain name you provided already exists.'
6. Replay apigatewayv2.create-domain-name-duplicate-bad-request.

Cause 2:
1. Boot the emulator; create a custom domain name <d> and an API with a 'dev' stage.
2. apigatewayv2 CreateRoutingRule(DomainName=<d>, Priority=100, Conditions=[{}], Actions=[{'InvokeApi': {'ApiId': <api-id>, 'Stage': 'dev'}}]).
3. Observe the BadRequestException message is 'Conditions[0] must specify exactly one of: MatchBasePaths, MatchHeaders.' instead of 'Invalid conditions for RoutingRule. Each condition can only be either matchBasePaths or matchHeaders.'
4. Replay apigatewayv2.create-routing-rule-missing-condition-rejected.
