Issue-Source: trace-sweep-divergence
Issue-Trace: securityhub-catalog-describe-standards
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityhub-catalog-describe-standards
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub|DescribeStandards|2xx->4xx|$.Standards
Issue-Key: securityhub|DescribeStandards|2xx->4xx|status
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# securityhub DescribeStandards returns 401 InvalidAccessException instead of the standards catalog

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStandards

## Description
The emulator gates the Security Hub CSPM standards catalog read on the calling account having an enabled hub, so a first-contact DescribeStandards call fails authorization instead of returning the catalog.

Cause 1 of 1: DescribeStandards is rejected with the modelled InvalidAccessException (HTTP 401) unless the (account, Region) pair already has a hub in state.

AWS: DescribeStandards is a GET /standards catalog read that "Returns a list of the available standards in Security Hub CSPM. For each standard, the results include the standard ARN, the name, and a description" (Smithy: SecurityHubAPIService 2018-10-26, restJson1; DescribeStandards http {method: GET, uri: /standards, code: 200}; output DescribeStandardsResponse with Standards: Standards and NextToken; modelled errors InternalException 500, InvalidAccessException 401, InvalidInputException 400). The recorded response for this fixture is HTTP 200 with a populated $.Standards list, each entry carrying Description, EnabledByDefault, Name, StandardsArn (arn:aws:securityhub:<region>::standards/...), StandardsManagedBy {Company, Product} and Provider — and the fixture is a single step with no preceding EnableSecurityHub, so the recording shows AWS serving the published catalog to an account that has not subscribed. The catalog is service metadata, not account data, which is consistent with the recorded 200.

Emulator: the replay got HTTP 401 and no Standards member at all ($.Standards missing). The handler computes the hub key from the request context and, when state.hubs has no entry for (account_id, region), returns InvalidAccessException with message "Account <id> is not subscribed to Security Hub CSPM in Region <region>". Because nothing in the corpus enables the hub first, every catalog read fails.

Why: a client that enumerates the available standards before (or instead of) enabling Security Hub — the normal way to discover StandardsArn values to pass to BatchEnableStandards — gets a 401 authorization failure where AWS returns the catalog. SDK retry/credential-refresh logic treats 401 InvalidAccessException as a caller/credential problem, so the client surfaces a misleading permissions error and never reaches the data; paginating the catalog is impossible.

Fix: the DescribeStandards handler's pre-flight authorization check. The catalog projection, Providers filtering and NextToken paging below it already match the modelled shape; the subscription precondition in front of them is what must go (or be narrowed to the hub-scoped operations), so the operation serves the Region-qualified catalog with status 200 regardless of whether the account has a hub. Status 200 and the presence/shape of Standards are the requirement; the recorded descriptions and standard names above are examples of catalog content.

Traces: securityhub-catalog-describe-standards#task#1 (expected 200, got 401; $.Standards missing).

## Evidence
Replay divergence (verdict=mismatch, divergence_key "service=securityhub|DescribeStandards|status=200->401|keys=$.Standards,status"):
- Expected: HTTP 200 with $.Standards = [{Name: "AI Security Best Practices v1.0.0", StandardsArn: "arn:aws:securityhub:us-west-2::standards/ai-security-best-practices/v/1.0.0", EnabledByDefault: false, Provider: "AWS", StandardsManagedBy: {Company: "AWS", Product: "Security Hub"}}, {Name: "AWS Foundational Security Best Practices v1.0.0", EnabledByDefault: true, ...}, {Name: "AWS Resource Tagging Standard v1.0.0", ...}, {Name: "Azure Foundational Security Best Practices ...", ...}, ...]
- Actual: HTTP 401, $.Standards missing (InvalidAccessException, the operation's modelled 401).
- Source of the 401: services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/describe_standards.rs lines 192-209 — hub_key(ctx) lookup in state.hubs, returning InvalidAccessException when absent. The module doc at lines 7-10 states the same precondition.
- Model: smithy_query(query="operation", operation="DescribeStandards") on services/service-com-amazonaws-securityhub-securityhubapiservice/smithy-model/com-amazonaws-securityhub-securityhubapiservice.json -> http {method GET, uri /standards, code 200}; errors InternalException(500)/InvalidAccessException(401)/InvalidInputException(400).

## Steps to Reproduce
1. Boot the emulator built from this checkout (securityhub is bootstrapped: service-com-amazonaws-securityhub-securityhubapiservice).
2. Replay wire fixture securityhub-catalog-describe-standards, step task#1 — a single GET /standards (DescribeStandards) with no prior EnableSecurityHub call, i.e. an account with no hub in state.
3. Observe HTTP 401 InvalidAccessException with no Standards member, where the fixture records HTTP 200 and a populated Standards list.
4. Equivalently, by hand: aws securityhub describe-standards against the emulator without first calling enable-security-hub.
