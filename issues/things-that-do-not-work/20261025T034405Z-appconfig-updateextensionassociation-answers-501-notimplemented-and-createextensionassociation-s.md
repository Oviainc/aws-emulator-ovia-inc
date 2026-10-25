Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.update-extension-association-nonexistent-rejected appconfig.update-extension-association-wrong-resource-type-rejected appconfig.create-extension-association-missing-required-parameter-rejected appconfig.extension-required-parameter-enforced-at-association
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.update-extension-association-nonexistent-rejected|run#19|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: appconfig.update-extension-association-wrong-resource-type-rejected|run#11|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: appconfig.create-extension-association-missing-required-parameter-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: appconfig.extension-required-parameter-enforced-at-association|run#25|AssertionError|assert missing_msg == (
Issue-Commit: 9181cd543fe84fac851a491e53a22f2746df95d8

# AppConfig UpdateExtensionAssociation answers 501 NotImplemented, and CreateExtensionAssociation's missing-required-parameter refusal does not use AWS's message template (2 causes)

> _Note: this issue was identified against the code base at commit `9181cd543fe84fac851a491e53a22f2746df95d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateExtensionAssociation

## Description
Two distinct net-new root causes on the AppConfig extension-association surface, both grounded in replayed behavioral checks. Numbered; close BOTH before submitting.

=== Cause 1 of 2: UpdateExtensionAssociation is an unimplemented scaffold stub, so every call is refused with 501 NotImplemented ===
AWS: UpdateExtensionAssociation is a real, fully modeled operation. Smithy: UpdateExtensionAssociation PATCH /extensionassociations/{ExtensionAssociationId}, success code 200, input ExtensionAssociationId (Id, pattern ^[a-z0-9]{4,7}$, required) + Parameters (ParameterValueMap, len 0..10), output shape ExtensionAssociation; modeled errors BadRequestException (HTTP 400), ResourceNotFoundException (HTTP 404), InternalServerException (HTTP 500). The two replayed checks exercise two of those modeled refusals: (a) an association id that matches the id pattern but names no association is refused ResourceNotFoundException / HTTP 404; (b) an update whose Parameters map drops a value for a parameter the extension's schema declares Required is refused BadRequestException / HTTP 400, the association's stored Parameters are left unchanged, and the same call that still supplies the required parameter succeeds and is visible through GetExtensionAssociation. The refusal messages are recorded behavior only, but they are request-derived: 'Parameter <name>  is required for extension <extensionId> version <version>, but a value has not been supplied.' for (b).
Emulator: the handler is the generated scaffold. services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension_association.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and its handle() body ends in Err(crate::scaffolded::Scaffolded::not_implemented("UpdateExtensionAssociation")), so every request — valid or invalid — is answered HTTP 501 with error code NotImplemented and the message 'operation UpdateExtensionAssociation is scaffolded but not implemented in this emulator'. Both checks observe code 'NotImplemented' where they expect 'ResourceNotFoundException' and 'BadRequestException'.
Why: a client cannot change an extension association's parameters at all. The whole point of an association's Parameters map is that it is tunable after creation (rotate a queue ARN, a topic, a role); against this emulator that call fails with a code no AWS SDK retry/branch logic models, so the error is neither a 404 to re-create on nor a 400 to fix the request from. The negative legs are worse than missing: an emulator that answers 501 silently passes input AWS refuses straight past the caller's error handling.
Fix: implement the handler in services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension_association.rs — resolve ExtensionAssociationId in the appconfig state (404 ResourceNotFoundException when absent), validate the merged Parameters map against the associated extension version's parameter schema (400 BadRequestException when a Required parameter has no value, leaving stored Parameters untouched), otherwise persist and return the ExtensionAssociation shape; then delete the SCAFFOLDED marker line.
Traces: appconfig.update-extension-association-nonexistent-rejected, appconfig.update-extension-association-wrong-resource-type-rejected.

=== Cause 2 of 2: CreateExtensionAssociation's 'no parameters supplied at all' refusal uses an invented sentence instead of AWS's template ===
AWS: CreateExtensionAssociation against an extension version that declares at least one Required parameter, with the Parameters member entirely ABSENT from the request, is refused BadRequestException / HTTP 400 with the request-derived template 'Extension <extensionName> version <extensionVersionNumber> has required parameters, but no parameters have been supplied' — it reports the condition (nothing was supplied) and names the extension and version, and it does NOT single out one parameter. Smithy: CreateExtensionAssociation POST /extensionassociations, success code 201, Parameters (ParameterValueMap, len 0..10) is optional in the model, errors BadRequestException 400 / ResourceNotFoundException 404 / ServiceQuotaExceededException 402 / InternalServerException 500. The template is recorded behavior only; both checks state it was verified against real AWS (us-west-2, 2026-09-07) and both assert it exactly, from two independently-named extensions.
Emulator: the status and the error code are already correct (400 BadRequestException), and the rejected call correctly creates no association, but the message is a different sentence shape that names a single parameter: e.g. 'Parameter MyRequiredParam is required by extension <extensionName> version 1.' and 'Parameter notificationtopic is required by extension <extensionName> version 1.' (run values, quoted as examples). The emulator collapses AWS's two distinct cases — 'no Parameters at all' and 'Parameters present but one required value missing' — onto the per-parameter sentence.
Why: the refusal text is the only place AppConfig distinguishes 'you sent no parameters' from 'you sent parameters but one is missing', and a client that parses or snapshot-tests the refusal (or surfaces it to an operator) sees a sentence AppConfig never emits; it also reads as though exactly one parameter were at fault when the request supplied none.
Fix: the required-parameter validation branch of the CreateExtensionAssociation handler must select AWS's template by case — when the request carries no Parameters member at all, render 'Extension <name> version <n> has required parameters, but no parameters have been supplied' from the resolved extension's Name and VersionNumber; keep a per-parameter sentence only for the case where Parameters is present and a required value is missing. This is the same class as the open issue 20261024T131534Z-appconfig-refusal-messages-on-the-tagging-pagination-hosted.md (request-derived message templates), but that issue names only TagResource/UntagResource/ListTagsForResource, the pagination lists, CreateHostedConfigurationVersion, CreateExperimentDefinition and UpdateExtension — it does not name CreateExtensionAssociation, so this path is not covered by it.
Traces: appconfig.create-extension-association-missing-required-parameter-rejected, appconfig.extension-required-parameter-enforced-at-association.

## Evidence
Behavioral replay, suite simparity-behavioral-ga-prod-v1. Identifiers, generated names and the account id below are the run's values and are examples; the requirement in Cause 1 is the status/code pair and in Cause 2 the message template.

Cause 1 — appconfig.update-extension-association-nonexistent-rejected [mismatch]: AssertionError: expected ResourceNotFoundException for nonexistent association 'rgy02nt', got 'NotImplemented'. appconfig.update-extension-association-wrong-resource-type-rejected [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'. Both calls reach UpdateExtensionAssociation, whose handler file still has the marker '//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented' and returns Scaffolded::not_implemented("UpdateExtensionAssociation") (services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/update_extension_association.rs:173). Note the suite did NOT mark these checks as blocked-by-an-already-filed stub, and no open issue names UpdateExtensionAssociation.

Cause 2 — appconfig.create-extension-association-missing-required-parameter-rejected [mismatch]: AssertionError: expected message 'Extension sim2real-behav-ext-cf707f3344d2 version 1 has required parameters, but no parameters have been supplied', got 'Parameter MyRequiredParam is required by extension sim2real-behav-ext-cf707f3344d2 version 1.'. appconfig.extension-required-parameter-enforced-at-association [mismatch]: AssertionError: unexpected refusal message: 'Parameter notificationtopic is required by extension sim2real-behav-ext-3f3b5c9b7b01 version 1.' against the same expected template. In both, code BadRequestException matched, and in the first the follow-up ListExtensionAssociations confirmed the rejected call leaked no association.

## Steps to Reproduce
Point an appconfig client at the emulator's data listener.

Cause 1:
1. create_extension(Name=N, Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name': 'notify', 'Uri': <an SQS queue ARN>}]}, Parameters={'RequiredParam': {'Required': True}, 'OptionalParam': {'Required': False}}).
2. create_application(Name=A); build app_arn = arn:aws:appconfig:<region>:<account>:application/<appId>.
3. create_extension_association(ExtensionIdentifier=<extId>, ResourceIdentifier=app_arn, Parameters={'RequiredParam': 'initial-value'}) -> note the association Id.
4. update_extension_association(ExtensionAssociationId=<a different id matching ^[a-z0-9]{4,7}$>, Parameters={}) -> expect 404 ResourceNotFoundException; emulator answers 501 NotImplemented.
5. update_extension_association(ExtensionAssociationId=<real id>, Parameters={'OptionalParam': 'opt-value'}) -> expect 400 BadRequestException and GetExtensionAssociation still showing {'RequiredParam': 'initial-value'}; emulator answers 501 NotImplemented.
6. update_extension_association(ExtensionAssociationId=<real id>, Parameters={'RequiredParam': 'updated-value', 'OptionalParam': 'opt-value'}) -> expect 200 and the new map readable; emulator answers 501 NotImplemented.

Cause 2:
1. create_extension(Name=N, Actions={'ON_DEPLOYMENT_COMPLETE': [{'Name': 'notify', 'Uri': <topic ARN>}]}, Parameters={'MyRequiredParam': {'Required': True}}).
2. create_application(Name=A); build the application ARN.
3. create_extension_association(ExtensionIdentifier=<extId>, ResourceIdentifier=<app arn>) with NO Parameters member -> 400 BadRequestException; compare only $.Message against 'Extension <N> version 1 has required parameters, but no parameters have been supplied'.

Re-replay every check in trace_ids to confirm both causes are closed.
