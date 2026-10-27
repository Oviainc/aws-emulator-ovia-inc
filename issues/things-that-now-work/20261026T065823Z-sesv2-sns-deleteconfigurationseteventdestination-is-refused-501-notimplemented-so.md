Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish
Issue-Key: sesv2__sns|DeleteConfigurationSetEventDestination|NotImplemented
Issue-Commit: 2e41c53f62212c879ccde9aca1b2ab6c6cf6a645

# sesv2__sns: DeleteConfigurationSetEventDestination is refused 501 NotImplemented, so an event destination can never be removed and the SNS topic-policy gate is unreachable

> _Note: this issue was identified against the code base at commit `2e41c53f62212c879ccde9aca1b2ab6c6cf6a645`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConfigurationSetEventDestination

## Description
One root cause.

=== Cause 1 of 1: DeleteConfigurationSetEventDestination is an unimplemented scaffold stub ===
AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) binds DeleteConfigurationSetEventDestination to DELETE /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations/{EventDestinationName}, output is an empty structure at HTTP 200, and the only modelled errors are BadRequestException (400), NotFoundException (404) and TooManyRequestsException (429) — there is no 501 in the contract. Recorded behavior: deleting an existing destination answers 200 and the name is gone from GetConfigurationSetEventDestinations immediately afterwards.
Emulator: every call is refused with botocore Code 'NotImplemented' and the message "operation DeleteConfigurationSetEventDestination is scaffolded but not implemented in this emulator" (the scaffold's 501 channel). The check dies there, mid-run, right after it has created the destination and before it can break the SNS topic policy, so everything past that point is ungraded.
Why: an event destination can never be removed or replaced by name — the normal rewire/teardown step of any SES eventing workflow — and the client receives a status outside the operation's modelled error set, which SDK error handling cannot map to any retry or recovery path. Any check or client that cleans up after itself is wedged at the first delete.
Fix: implement the handler in services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_configuration_set_event_destination.rs over the existing configuration_sets store in src/state.rs (the same set.event_destinations map src/ops/create_configuration_set_event_destination.rs inserts into): 404 NotFoundException with this service's shared wording when the set or the destination name is absent, otherwise remove the entry and answer 200 with an empty body, and drop the SCAFFOLDED marker.
Traces: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish.

Notes for the fixer. (a) This is the same unimplemented operation that the auto-filed sesv2 scaffold-stub issue for DeleteConfigurationSetEventDestination names (raised from sesv2.configuration-set-event-destination-lifecycle); it is recorded separately here because the sesv2__sns check above carries its own replay key and is not covered by that issue. (b) The check is an unblessed DRAFT (never replayed against real AWS, no .gt.jsonl oracle), so only the 501 refusal above is grounded in this run. Hypothesis the check will grade next, NOT part of this cause: after the delete works, it expects CreateConfigurationSetEventDestination against an SNS topic whose policy explicitly denies ses.amazonaws.com:Publish to be refused 400 BadRequestException with "Access denied to SNS topic <TOPIC_ARN>. Verify the SNS topic policy grants Amazon SES the sns:Publish permission." (fixed text around the request's TopicArn), with the destination not created, and the identical call to succeed once the policy is flipped to Allow — the emulator's create handler validates no topic policy today (src/ops/create_configuration_set_event_destination.rs lines 161-218).

## Evidence
Suite: 3/113 passed. Surviving sesv2__sns failure:

[error] sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteConfigurationSetEventDestination operation: "operation DeleteConfigurationSetEventDestination is scaffolded but not implemented in this emulator". Expected: HTTP 200 with an empty body, followed by the destination being absent from GetConfigurationSetEventDestinations (the check asserts "<name> survived DeleteConfigurationSetEventDestination, so the negative and restore legs would not be using the identical call"). The run reaches this call after CreateConfigurationSet and a successful CreateConfigurationSetEventDestination against an allow-policy SNS topic, so the create path works and only the delete is unwritten.

## Steps to Reproduce
Replay the behavioral suite and re-run sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish: CreateConfigurationSet, CreateConfigurationSetEventDestination with an SnsDestination (any topic), then DeleteConfigurationSetEventDestination for that ConfigurationSetName/EventDestinationName; expect HTTP 200 and the destination gone from GetConfigurationSetEventDestinations, observe 501 'NotImplemented' ("operation DeleteConfigurationSetEventDestination is scaffolded but not implemented in this emulator").
