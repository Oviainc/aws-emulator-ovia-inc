Issue-Source: trace-sweep-divergence
Issue-Trace: account-alternate-contact-validation-errors
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: account-alternate-contact-validation-errors account-contact-information-validation-errors
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account|PutAlternateContact|4xx->4xx|$.message
Issue-Key: account|PutContactInformation|4xx->4xx|$.message
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# account ValidationException message ignores the Smithy pattern-constraint format

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAlternateContact

## Description
Two causes, same fault on service `account`: a request member that violates its modelled @pattern is correctly refused with HTTP 400 ValidationException, but the message the emulator builds from the request uses a different construction rule than AWS's. Both must be closed before this issue is submitted.

Cause 1 of 2: PutAlternateContact.EmailAddress pattern violation.
  AWS: Smithy: PutAlternateContact.EmailAddress is required, @length 1..254, pattern ^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$; the modelled failure is com.amazonaws.account#ValidationException (@error client, HTTP 400) whose `message` member is documented as "The message that informs you about what was invalid about the request." The message is BUILT FROM THE REQUEST: the recorded response states the violated ECMA-262 regex and the offending input value, in the form `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` — e.g. `[ECMA 262 regex "^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$" does not match input string "not-a-valid-email"]` for input "not-a-valid-email". Status 400 and the ValidationException error code match.
  Emulator: same 400 and same error shape, but the message follows the generic Coral-style rule instead: `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` (recorded example: "1 validation error detected: Value 'not-a-valid-email' at 'emailAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$").
  Why: a client that surfaces or matches on the constraint-violation text for this service sees a message shaped by a different rule — the member is also named in lowerCamel wire form rather than the ECMA-262 phrasing AWS uses — so log scraping and message-based assertions written against AWS do not hold.
  Fix: the shared request-validation layer that renders pattern-constraint violations into ValidationException.message for this service (not the handler): emit the `[ECMA 262 regex "<pattern>" does not match input string "<value>"]` construction.
  Traces: account-alternate-contact-validation-errors#task#2 (cluster also covering task#3).

Cause 2 of 2: PutContactInformation.ContactInformation.PhoneNumber pattern violation.
  AWS: Smithy: ContactInformation.PhoneNumber targets ContactInformationPhoneNumber, required, @length 1..20, pattern ^[+][\s0-9()-]+$; failure is ValidationException, HTTP 400. Same request-derived message rule: `[ECMA 262 regex "^[+][\s0-9()-]+$" does not match input string "not-a-phone-number"]`.
  Emulator: 400 with "1 validation error detected: Value 'not-a-phone-number' at 'contactInformation.phoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[+][\s0-9()-]+$" — the same wrong construction rule, here for a nested member path.
  Why: as above; the divergence is systematic across the service's pattern-constrained members (top-level and nested alike), so no client can rely on the service's documented constraint-violation text.
  Fix: the same request-validation message rendering, covering nested member paths too.
  Traces: account-contact-information-validation-errors#task#2.

## Evidence
Fixture account-alternate-contact-validation-errors, step task#2 (PutAlternateContact; cluster also covering task#3): expected 400 / actual 400, $.message: value_mismatch — expected '[ECMA 262 regex "^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$" does not match input string "not-a-valid-email"]', got "1 validation error detected: Value 'not-a-valid-email' at 'emailAddress' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\s]*[\w+=.#|!&-]+@[\w.-]+\.[\w]+[\s]*$".
Fixture account-contact-information-validation-errors, step task#2 (PutContactInformation): expected 400 / actual 400, $.message: value_mismatch — expected '[ECMA 262 regex "^[+][\s0-9()-]+$" does not match input string "not-a-phone-number"]', got "1 validation error detected: Value 'not-a-phone-number' at 'contactInformation.phoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[+][\s0-9()-]+$".
Status (400) and the ValidationException error shape agree in both; only the message construction differs. The messages quoted here are examples of the rule — the requirement is the rule itself (the violated @pattern and the offending input value rendered in the ECMA-262 form), not the recorded text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture account-alternate-contact-validation-errors, step task#2: POST /putAlternateContact with AlternateContactType=SECURITY, a valid Name/Title/PhoneNumber and EmailAddress="not-a-valid-email". Observe 400 ValidationException whose message uses the "1 validation error detected: ..." construction instead of '[ECMA 262 regex "..." does not match input string "not-a-valid-email"]'.
3. Fixture account-contact-information-validation-errors, step task#2: POST /putContactInformation with ContactInformation.PhoneNumber="not-a-phone-number" (other members valid). Observe the same wrong message construction for the nested member path.
