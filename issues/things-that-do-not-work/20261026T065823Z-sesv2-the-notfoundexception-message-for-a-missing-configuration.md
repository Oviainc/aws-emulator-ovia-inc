Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.configuration-set-event-destination-notfound
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.configuration-set-event-destination-notfound|run#16|AssertionError|assert err == expected, f'expected {expected} creating an event destination on a missing configuration set, got {err!r}'
Issue-Commit: 2e41c53f62212c879ccde9aca1b2ab6c6cf6a645

# sesv2: the NotFoundException message for a missing configuration set omits the angle brackets AWS puts around the name

> _Note: this issue was identified against the code base at commit `2e41c53f62212c879ccde9aca1b2ab6c6cf6a645`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
One root cause.

=== Cause 1 of 1: "Configuration set <name> does not exist." is rendered without the angle brackets ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) models CreateConfigurationSetEventDestination (POST /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations) and GetConfigurationSetEventDestinations (GET, same path) with NotFoundException at HTTP 404, "The resource you attempted to access doesn't exist." Recorded behavior: naming a configuration set that does not exist is refused 404 NotFoundException with the message "Configuration set <NAME> does not exist." — the fixed text is "Configuration set <" ... "> does not exist." and the only variable part is the request's ConfigurationSetName, which AWS wraps in angle brackets (the same bracketing SES uses in its other resource-not-found sentences).
Emulator: the status (404) and the code (NotFoundException) are right, but the message is "Configuration set NAME does not exist." — the brackets around the name are missing. The wording is built once in src/state.rs (pub fn configuration_set_not_found, lines 1027-1029: format!("Configuration set {configuration_set_name} does not exist.")) and is shared by every operation that takes a ConfigurationSetName, so the same divergence is served by CreateConfigurationSetEventDestination, GetConfigurationSetEventDestinations and every sibling that reports a missing set (src/ops/create_configuration_set_event_destination.rs lines 174-183 is the call site this check hits first).
Why: the error message is part of the contract a client sees. Code and tests that match or parse SES's not-found sentence (extracting the bracketed resource name, or asserting the message verbatim) fail against the emulator although the request, status and error code are otherwise identical to AWS's, and an operator diffing logs between AWS and the emulator sees a different sentence for the same condition.
Fix: the shared message builder in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/state.rs (configuration_set_not_found) — render the name inside angle brackets so every operation that reports a missing configuration set emits AWS's sentence.
Traces: sesv2.configuration-set-event-destination-notfound.

## Evidence
Suite: 3/113 passed. Surviving sesv2 failure for this cause:

[mismatch] sesv2.configuration-set-event-destination-notfound — AssertionError: expected ('NotFoundException', 'Configuration set <sim2real-behav-cfgset-missing-9fe01a21b0f1> does not exist.') creating an event destination on a missing configuration set, got ('NotFoundException', 'Configuration set sim2real-behav-cfgset-missing-9fe01a21b0f1 does not exist.'). The name in both strings is the per-run unique name the check generated; the requirement is the bracketing, not that literal. The check also asserts the identical (code, message) pair from GetConfigurationSetEventDestinations on the same missing name, and that the identical create succeeds once the configuration set exists. Emulator source: src/state.rs:1027-1029.

## Steps to Reproduce
Replay the behavioral suite and re-run sesv2.configuration-set-event-destination-notfound: call CreateConfigurationSetEventDestination with a ConfigurationSetName that does not exist (any valid name), then GetConfigurationSetEventDestinations on the same name; expect HTTP 404 NotFoundException with the message "Configuration set <that name> does not exist." from both, observe the same code and status with the name unbracketed.
