Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatchomni__iam.deploy-cloudwatch-omni-estate-provisioned-and-torn-down
Issue-Key: cloudformation__cloudwatchomni|ListDomains|NotImplemented
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# cloudformation__cloudwatchomni: cloudwatchomni ListDomains is an unimplemented scaffold stub answering 501 NotImplemented, so no CloudWatch Omni estate can be read back after a CloudFormation deploy

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomains

## Description
Service token note: the suite lists this check under the composite heading `### cloudformation__cloudwatchomni`; the defect itself is in the cloudwatchomni service crate (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend). Both constituent services are bootstrapped (service-com-amazonaws-cloudformation-cloudformation and service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend are in the bootstrapped set; nothing is pending), so this is filed as a fidelity finding.

Cause 1 of 1: cloudwatchomni ListDomains is never implemented and answers 501 NotImplemented.

AWS: Smithy model com.amazonaws.cloudwatchomni#ListDomains is a @readonly, @paginated operation (inputToken nextToken, outputToken nextToken, pageSize maxResults, items items). Input: `maxResults` (Integer, @range 1..100, documented default 100), `nextToken` (NextToken, @length 1..2048). Output: `items` (DomainSummaryList) is @required, `nextToken` (NextToken, @length 1..2048) is present only when a further page exists. Modeled errors are AccessDeniedException (403), ValidationException (400), ThrottlingException (429) and InternalServerException (500) — there is NO modeled 501 and no NotImplemented error shape, so 501/NotImplemented is off-contract for every input. The operation's own documentation states it "Returns the caller's domains: the account-scoped domain and the organization-scoped domain, if either exists. At most two domains are returned" — i.e. an account holding no domain must get HTTP 200 with `items` present and empty, not a failure.

Emulator: every ListDomains call answers HTTP 501 with error code `NotImplemented` and the message `operation ListDomains is scaffolded but not implemented in this emulator`. The handler is still the generated scaffold: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/list_domains.rs line 164 returns `Err(crate::scaffolded::Scaffolded::not_implemented("ListDomains"))`, and the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker is still on line 4.

Why: a client cannot enumerate CloudWatch Omni domains at all. The deploy check cannot even establish its precondition (an account with no domain reads as a hard ClientError instead of an empty `items` list), and after the stack is created/updated/deleted there is no way to confirm the Domain was provisioned or removed, so the whole AWS::CloudWatch::Domain lifecycle through CloudFormation is unverifiable. Because 501/NotImplemented is not a modeled error, botocore surfaces it as an unmodeled ClientError and no retry/paging logic in any SDK can recover; code that treats "no domains" as the normal empty-account case breaks outright.

Fix: implement the handler in services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/list_domains.rs — read the domain records from the cloudwatchomni service state via crate::state::service_state, project each to a DomainSummary, honour maxResults/nextToken paging as the @paginated trait declares, always populate the required `items` member (empty list when the account holds no domain), and delete the SCAFFOLDED marker line at the top of the file.

Traces: cloudformation__cloudwatchomni__iam.deploy-cloudwatch-omni-estate-provisioned-and-torn-down fails on its first call, `paginate(cwo, "list_domains", "items")`.

Related but not covering: the open issue 20261029T175624Z-cloudwatchomni-createintegration-stub.md covers CreateIntegration only; no open issue names ListDomains.

## Evidence
Expected (AWS, per the Smithy model): HTTP 200 and a ListDomainsOutput whose @required `items` member is a (possibly empty) DomainSummaryList, with `nextToken` only when another page exists; failures limited to the four modeled errors (403 AccessDeniedException / 400 ValidationException / 429 ThrottlingException / 500 InternalServerException).

Actual (emulator): HTTP 501, Error.Code `NotImplemented`, Error.Message `operation ListDomains is scaffolded but not implemented in this emulator`. Message anatomy: the fixed text is `operation ` ... ` is scaffolded but not implemented in this emulator`; the only variable part is the operation name `ListDomains` taken from the request. Neither the code nor the status is in the operation's modeled error set.

botocore traceback from the replay:
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDomains operation: operation ListDomains is scaffolded but not implemented in this emulator

Source evidence: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/list_domains.rs:4 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and :164 (`Err(crate::scaffolded::Scaffolded::not_implemented("ListDomains"))`).

## Steps to Reproduce
1. Boot the emulator and point a botocore `cloudwatchomni` client at it.
2. Call ListDomains on an account that holds no CloudWatch Omni domain, e.g. `client.get_paginator("list_domains").paginate()` or `client.list_domains()`.
3. Observe HTTP 501 / `NotImplemented` instead of HTTP 200 with `items: []`.
4. Equivalent end-to-end: replay behavioral check `cloudformation__cloudwatchomni__iam.deploy-cloudwatch-omni-estate-provisioned-and-torn-down`; it errors on its first cloudwatchomni call (the `prior = paginate(cwo, "list_domains", "items")` precondition) before any stack is created.
