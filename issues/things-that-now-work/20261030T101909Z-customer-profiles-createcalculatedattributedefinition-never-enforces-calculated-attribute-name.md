Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.concurrent-calculated-attribute-creation-exactly-one-wins
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.concurrent-calculated-attribute-creation-exactly-one-wins|run#16|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)}: {results}"
Issue-Commit: 26c292b5aced43fd01c633b76d9a8d262851cd06

# customer-profiles CreateCalculatedAttributeDefinition never enforces calculated-attribute name uniqueness: every concurrent racer is answered 501 NotImplemented, so no winner is created and no loser is refused BadRequestException

> _Note: this issue was identified against the code base at commit `26c292b5aced43fd01c633b76d9a8d262851cd06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCalculatedAttributeDefinition

## Description
Cause 1 of 1: CreateCalculatedAttributeDefinition does not implement the create-or-conflict contract for a calculated attribute name inside a domain, so neither the winning create nor the duplicate-name refusal is reachable. Failing check: customer-profiles.concurrent-calculated-attribute-creation-exactly-one-wins.

AWS: CreateCalculatedAttributeDefinition (Smithy: POST /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}, success code 200; required input members DomainName, CalculatedAttributeName, AttributeDetails, Statistic; modeled errors AccessDeniedException 403, BadRequestException 400, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429). CalculatedAttributeName is an httpLabel keyed within the domain (@length 1-255, @pattern ^[a-zA-Z_][a-zA-Z_0-9-]*$), so the (DomainName, CalculatedAttributeName) pair identifies one definition: the first create of a name succeeds with 200 and a CreateCalculatedAttributeDefinitionResponse echoing CalculatedAttributeName, Statistic, AttributeDetails, CreatedAt/LastUpdatedAt, and any create of a name that already exists in the domain is refused with the modeled client error BadRequestException (HTTP 400). Under N concurrent creates of the same name, exactly one is admitted and the other N-1 are refused BadRequestException; afterwards exactly one definition with that name is readable through ListCalculatedAttributeDefinitions / GetCalculatedAttributeDefinition. The duplicate-name refusal message is BadRequestException's fixed sentence for an already-existing calculated attribute; its only variable part is the calculated-attribute name taken from the request path (recorded behavior only for the exact sentence — the model documents the error class, not the wording).

Emulator: every call to the operation is answered with the scaffold refusal — HTTP 501, error code NotImplemented, message "operation CreateCalculatedAttributeDefinition is scaffolded but not implemented in this emulator". With 6 racers submitting the same CalculatedAttributeName the replay observed 6 x ('error', 'NotImplemented'): 0 winners, 0 BadRequestException losers, and no definition persisted for the subsequent ListCalculatedAttributeDefinitions / GetCalculatedAttributeDefinition reads. NotImplemented is not a member of the operation's modeled error set and 501 is not one of its modeled statuses.

Why: a client that creates a calculated attribute gets a 501 that botocore surfaces as a non-retryable, unmodeled ClientError, so no calculated attribute can ever be defined in a domain. Worse for correctness-sensitive callers: the uniqueness contract is unobservable, so code that treats BadRequestException as "someone else already created it" (the standard create-or-adopt idiom, and the behavior relied on by concurrent provisioners such as IaC retries) can neither adopt nor create, and code that counts on exactly one definition surviving a race has nothing to assert against.

Fix: the operation's handler (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/create_calculated_attribute_definition.rs) plus the domain state that keys calculated-attribute definitions by (DomainName, CalculatedAttributeName) — the insert must be a single atomic check-and-insert so that concurrent creates of one name admit exactly one and refuse the rest BadRequestException (400), and the refusal path must be in the handler's validation, not the scaffold fallback. Note the already-open stub issue for this operation only asks for the operation to exist; this issue additionally requires the name-uniqueness / duplicate-name-refusal semantics and its atomicity under concurrency.

Traces: customer-profiles.concurrent-calculated-attribute-creation-exactly-one-wins (6 concurrent creates of one name; assertion "expected exactly 1 winner, got 0").

## Evidence
Expected (AWS): of 6 concurrent CreateCalculatedAttributeDefinition calls with the same DomainName + CalculatedAttributeName, 1 -> HTTP 200 CreateCalculatedAttributeDefinitionResponse (CalculatedAttributeName echoed, Statistic=SUM), 5 -> HTTP 400 with Error.Code "BadRequestException"; then ListCalculatedAttributeDefinitions(DomainName) contains exactly one Item with that CalculatedAttributeName and GetCalculatedAttributeDefinition returns it with Statistic=SUM.

Actual (emulator): all 6 calls -> HTTP 501, Error.Code "NotImplemented", Error.Message "operation CreateCalculatedAttributeDefinition is scaffolded but not implemented in this emulator". Replay output: AssertionError: expected exactly 1 winner, got 0: [('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented')]. The terminal reads (ListCalculatedAttributeDefinitions / GetCalculatedAttributeDefinition) are never reached because nothing was persisted.

Note on message parts: "NotImplemented"/501 is wholly emulator-authored and not in the operation's modeled error set (AccessDeniedException, BadRequestException, InternalServerException, ResourceNotFoundException, ThrottlingException). For the duplicate refusal, the fixed text is BadRequestException's already-exists sentence and the only request-derived value is the calculated-attribute name from the {CalculatedAttributeName} path label.

## Steps to Reproduce
1. CreateDomain(DomainName=<unique>, DefaultExpirationDays=30).
2. PutProfileObjectType(DomainName=<unique>, ObjectTypeName='Orders', Fields={'Amount': {ContentType: NUMBER, Source: '_source.amount', Target: '_profile.Attributes.amount'}}, Keys={'uniqueKey': [{StandardIdentifiers: [PROFILE, UNIQUE], FieldNames: ['Amount']}]}).
3. From 6 threads simultaneously call CreateCalculatedAttributeDefinition(DomainName=<unique>, CalculatedAttributeName=<same name in all 6>, AttributeDetails={'Attributes': [{'Name': 'Amount'}], 'Expression': '{Orders.Amount}'}, Statistic='SUM').
4. Collect the 6 outcomes: AWS yields exactly 1 success and 5 BadRequestException (400); the emulator yields 6 x 501 NotImplemented.
5. Call ListCalculatedAttributeDefinitions(DomainName=<unique>) and GetCalculatedAttributeDefinition(DomainName=<unique>, CalculatedAttributeName=<name>): AWS shows exactly one persisted definition with Statistic=SUM; the emulator has none.
