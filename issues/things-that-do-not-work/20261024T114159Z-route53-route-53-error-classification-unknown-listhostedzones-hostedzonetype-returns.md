Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.list-hosted-zones-private-type-filter-lists-only-private-zones route53.get-change-not-found
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53.list-hosted-zones-private-type-filter-lists-only-private-zones|run#17|AssertionError|assert code == "InvalidInput", f"an unknown HostedZoneType: got {code}: {message!r}"
Issue-Key: route53.get-change-not-found|run#11|AssertionError|assert err["Message"] == f"Could not find resource with ID: {bogus_id}", err
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Route 53 error classification: unknown ListHostedZones hostedzonetype returns ValidationException instead of InvalidInput, and GetChange's NoSuchChange message is not the AWS form

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListHostedZones

## Description
Two causes in how the route53 control plane classifies and words client faults. Both operations are implemented and otherwise behave; what diverges is the error identity a client branches on.

Cause 1 of 2: an unrecognized `hostedzonetype` query value is refused as a generic modeled-constraint `ValidationException` instead of Route 53's `InvalidInput`.
AWS: Smithy model (services/service-com-amazonaws-route53-awsdnsv20130401/smithy-model/com-amazonaws-route53-awsdnsv20130401.json) — ListHostedZones input member `HostedZoneType` is bound to httpQuery `hostedzonetype` and targets the `HostedZoneType` enum; the operation models `InvalidInput` (HTTP 400) and `ValidationException` is NOT in its error list (nor in the service's common errors). Route 53 is a restXml service that reports an unusable query value with its own `InvalidInput` 400, so a value outside the enum set must come back as Error.Code `InvalidInput`.
Emulator: HTTP 400 with Error.Code `ValidationException` and message (example) "1 validation error detected: Value 'SharedHostedZone' at 'hostedZoneType' failed to satisfy constraint: Member must satisfy enum value set: [PrivateHostedZone]" — the request is rejected by the generic pre-handler constraint walk under a code the operation does not model.
Why: a client cannot catch this with the modeled exception type; botocore raises a generic ClientError whose code matches no member of the route53 error shape set, so `except route53.exceptions.InvalidInput` never fires and retry/validation logic written against AWS misroutes the failure.
Fix: the ListHostedZones handler / its query-parameter validation — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/list_hosted_zones.rs: an out-of-set `hostedzonetype` must be refused by the operation as modeled `InvalidInput` (400) rather than by the shared modeled-constraint validator as `ValidationException`.
Traces: route53.list-hosted-zones-private-type-filter-lists-only-private-zones.

Cause 2 of 2: GetChange on an unknown change id returns the right class with a different message rule.
AWS: Smithy model — GetChange models `NoSuchChange` (HTTP 404). The recorded AWS message is the resource-lookup form built from the request: "Could not find resource with ID: <the Id submitted>".
Emulator: Error.Code `NoSuchChange`, Type `Sender` (class and status correct), message "no change exists with the ID CE010809CB51C459BA086" — a different rule (prose) for the same fault; the id is echoed, but not in the "Could not find resource with ID: <id>" form the request-derived message uses.
Why: narrow — the code and status are right, so well-written clients are unaffected; tooling and tests that assert the documented not-found message (or parse the id out of it) diverge.
Fix: the not-found branch of the GetChange handler — services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/get_change.rs.
Traces: route53.get-change-not-found.

## Evidence
Cause 1 — AssertionError: an unknown HostedZoneType: got ValidationException: "1 validation error detected: Value 'SharedHostedZone' at 'hostedZoneType' failed to satisfy constraint: Member must satisfy enum value set: [PrivateHostedZone]"; expected Error.Code `InvalidInput` (HTTP 400). ValidationException is not a modeled error of ListHostedZones.
Cause 2 — AssertionError: {'Message': 'no change exists with the ID CE010809CB51C459BA086', 'Code': 'NoSuchChange', 'Type': 'Sender'}; expected Message "Could not find resource with ID: CE010809CB51C459BA086" with the same Code/status (404).

## Steps to Reproduce
1. Call route53.list_hosted_zones(HostedZoneType="SharedHostedZone") (any value outside the modeled enum) and read Error.Code: ValidationException, expected InvalidInput.
2. Call route53.get_change(Id="<well-formed but nonexistent change id>") and read Error.Message: "no change exists with the ID <id>", expected "Could not find resource with ID: <id>" (Code NoSuchChange, HTTP 404, both already correct).
