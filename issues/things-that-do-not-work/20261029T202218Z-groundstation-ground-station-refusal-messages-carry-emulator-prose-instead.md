Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-contact-read-and-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-contact-read-and-error-paths groundstation-dataflow-endpoint-group-lifecycle groundstation-mission-profile-error-paths groundstation-reference-data-ground-stations-and-satellites
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|DescribeContactVersion|4xx->4xx|$.message
Issue-Key: groundstation|UpdateContact|4xx->4xx|$.message
Issue-Key: groundstation|CreateDataflowEndpointGroup|4xx->4xx|$.message
Issue-Key: groundstation|GetDataflowEndpointGroup|4xx->4xx|$.message
Issue-Key: groundstation|DeleteMissionProfile|4xx->4xx|$.message
Issue-Key: groundstation|ListAntennas|4xx->4xx|$.message
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Ground Station refusal messages carry emulator prose instead of the fixed text AWS emits for each error class

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeContactVersion

## Description
Six net-new wire causes, all in groundstation, all the same fault: the refusal carries the correct error code and HTTP status but the message is the emulator's own prose rather than the text AWS emits for that error class. A message is part of the requirement, so each is a divergence; they are bundled because they share one service, and each must be closed separately.

Related open issue: 20261029T182737Z-groundstation-four-ground-station-refusals-carry-invented-message-text.md describes the same SHAPE of defect for four BEHAVIORAL checks, and its cause 2 is the same securityDetails message as cause 3 below. That issue carries no replay key for this wire step, so it does not cover it; cause 3 is filed here with its fixture binding so a fixer can re-replay it.

Cause 1 of 6: DescribeContactVersion for a contactId/versionId pair that resolves to no version.
AWS: Smithy: DescribeContactVersion.contactId (@required @httpLabel Uuid) and .versionId (@required @httpLabel integer, range 1..128); errors ResourceNotFoundException (434), InvalidParameterException (431). Recorded behavior: the miss is refused with HTTP 434 and the fixed message 'Contact version not found' — the refusal is phrased about the VERSION, which is what the operation addresses; nothing is interpolated.
Emulator: right code and right status (434) but the message is 'Contact not found', the contact-level wording, which misreports which thing was missing.
Why: the two messages name different objects. A caller that distinguishes 'this contact does not exist' from 'this contact has no such version' by message — the only way to tell them apart, since the code and status are identical — concludes the contact is gone and stops polling or re-creating versions.
Fix: give DescribeContactVersion its own not-found message in its handler (services/service-com-amazonaws-groundstation-groundstation/src/ops/describe_contact_version.rs) rather than reusing the contact-level helper in src/state.rs.
Traces: groundstation-contact-read-and-error-paths#task#3.

Cause 2 of 6: UpdateContact on a contactId that names no contact.
AWS: Smithy: UpdateContact.contactId is an @required @httpLabel Uuid; errors ResourceNotFoundException (434), InvalidParameterException (431), ResourceLimitExceededException (429). Recorded behavior: HTTP 434 with the message template 'Contact not found: {contactId}' — the fixed text is 'Contact not found: ' and the only variable part is the contactId taken from the request path.
Emulator: HTTP 434 with 'No contact found with ID {contactId} for customer {accountId}' — different fixed text, and it additionally leaks the account id, which is not part of the template at all.
Why: the message is what a client surfaces and what log-greps and error-mapping layers key on, so neither matches; the extra account id also makes the text vary with the environment, so even a substring match on the AWS wording fails.
Fix: the contact-lookup miss path in the UpdateContact handler (src/ops/update_contact.rs) must emit 'Contact not found: <contactId>' and drop the customer/account fragment.
Traces: groundstation-contact-read-and-error-paths#task#5.

Cause 3 of 6: CreateDataflowEndpointGroup with an endpointDetails entry that omits securityDetails.
AWS: Smithy: EndpointDetails.securityDetails is NOT @required (SecurityDetails itself requires subnetIds, securityGroupIds and roleArn), but the service refuses the dataflow-endpoint form that omits it; errors InvalidParameterException (431). Recorded behavior: HTTP 431 with the fixed message 'EndpointDetails securityDetails cannot be null' — fixed text only, with no list index or member path interpolated.
Emulator: right code and status, but the message is 'endpointDetails[0] is a dataflow endpoint and names no securityDetails; a dataflow endpoint requires the subnetIds, securityGroupIds and roleArn Ground Station connects its streams with' — prose AWS never emits, which also interpolates the list index.
Why: callers and SDK wrappers surface this message verbatim; assertions, log-greps and user-facing error mapping keyed on AWS's sentence do not match.
Fix: the securityDetails presence gate in the CreateDataflowEndpointGroup handler (src/ops/create_dataflow_endpoint_group.rs) must emit AWS's fixed message.
Traces: groundstation-dataflow-endpoint-group-lifecycle#task#6.

Cause 4 of 6: GetDataflowEndpointGroup on a group id that has been deleted (or never existed).
AWS: Smithy: GetDataflowEndpointGroup.dataflowEndpointGroupId is an @required @httpLabel Uuid; errors ResourceNotFoundException (434), InvalidParameterException (431). Recorded behavior: HTTP 434 with the fixed message 'The specified group does not exist.' — entirely fixed text, with no id interpolated.
Emulator: HTTP 434 with 'DataflowEndpointGroup not found' — the emulator's own shorthand.
Why: the post-delete read is the standard way a client confirms a teardown completed; it matches on AWS's sentence and sees different text, so the confirmation step fails even though the delete worked.
Fix: the group-lookup miss path in the GetDataflowEndpointGroup handler (src/ops/get_dataflow_endpoint_group.rs).
Traces: groundstation-dataflow-endpoint-group-lifecycle#teardown#10.

Cause 5 of 6: DeleteMissionProfile on a missionProfileId that names no mission profile.
AWS: Smithy: DeleteMissionProfile.missionProfileId is an @required @httpLabel Uuid; errors ResourceNotFoundException (434), InvalidParameterException (431). Recorded behavior: HTTP 434 with the template 'Mission Profile {missionProfileId} not found' — the fixed text is 'Mission Profile ' … ' not found' and the single variable part is the missionProfileId from the request path.
Emulator: HTTP 434 with 'MissionProfile not found' — different fixed text, and it drops the id the template requires, so the message no longer says WHICH profile was missing.
Why: a client deleting several profiles concurrently cannot attribute the failure without the id, and the fixed words differ too, so message-based handling and operator log searches both miss.
Fix: the mission-profile lookup miss path in the DeleteMissionProfile handler (src/ops/delete_mission_profile.rs), interpolating the requested id into AWS's template.
Traces: groundstation-mission-profile-error-paths#task#4.

Cause 6 of 6: ListAntennas for a groundStationId that names no ground station.
AWS: Smithy: ListAntennas.groundStationId is an @required @httpLabel GroundStationName (length 4..97, pattern ^[ a-zA-Z0-9-._:=]{4,97}$); the operation's ONLY modeled client error is InvalidParameterException (431) — there is no ResourceNotFoundException on it, so an unresolvable ground station is a parameter error by construction. Recorded behavior: HTTP 431 with the fixed message 'Invalid request: Ground station is invalid, does not exist, or you do not have access to it' — fixed text only; it deliberately does NOT echo the id and deliberately conflates nonexistence with lack of access.
Emulator: right code and status, but the message is 'No ground station found with id {groundStationId}', which both rewords the refusal and echoes the requested name.
Why: AWS's wording is the one that tells the caller the id may be valid but inaccessible; the emulator's asserts it does not exist, so a client (or an operator) diagnoses a typo where the real cause may be authorization, and message-keyed handling does not match.
Fix: the ground-station resolution gate in the ListAntennas handler (src/ops/list_antennas.rs) must emit AWS's fixed message.
Traces: groundstation-reference-data-ground-stations-and-satellites#task#2.

## Evidence
Cause 1 — fixture groundstation-contact-read-and-error-paths step task#3, DescribeContactVersion (status 434 on both sides):
  expected message: 'Contact version not found'
  actual message:   'Contact not found'

Cause 2 — fixture groundstation-contact-read-and-error-paths step task#5, UpdateContact (status 434 on both sides):
  expected message: 'Contact not found: 91d8263b-81bd-4215-8e3b-10e64dde96b4'
  actual message:   'No contact found with ID 91d8263b-81bd-4215-8e3b-10e64dde96b4 for customer 000000000000'
  The uuid is the contactId the run requested — it FILLS the template's one variable slot; the divergence is the fixed text ('Contact not found: ' vs 'No contact found with ID … for customer …') plus the account id the template does not contain. The account number 000000000000 is a per-environment value and is not part of any requirement.

Cause 3 — fixture groundstation-dataflow-endpoint-group-lifecycle step task#6, CreateDataflowEndpointGroup (status 431 on both sides):
  expected message: 'EndpointDetails securityDetails cannot be null'
  actual message:   'endpointDetails[0] is a dataflow endpoint and names no securityDetails; a dataflow endpoint requires the subnetIds, securityGroupIds and roleArn Ground Station connects its streams with'

Cause 4 — fixture groundstation-dataflow-endpoint-group-lifecycle step teardown#10, GetDataflowEndpointGroup (status 434 on both sides):
  expected message: 'The specified group does not exist.'
  actual message:   'DataflowEndpointGroup not found'

Cause 5 — fixture groundstation-mission-profile-error-paths step task#4, DeleteMissionProfile (status 434 on both sides):
  expected message: 'Mission Profile c3e1bd40-8ecd-4f3f-82ee-f66ead24c5c9 not found'
  actual message:   'MissionProfile not found'
  The uuid is the missionProfileId the run requested — it fills the template's one variable slot; the requirement is the template 'Mission Profile {missionProfileId} not found', not that literal id.

Cause 6 — fixture groundstation-reference-data-ground-stations-and-satellites step task#2, ListAntennas (status 431 on both sides):
  expected message: 'Invalid request: Ground station is invalid, does not exist, or you do not have access to it'
  actual message:   'No ground station found with id Nonexistent Station'
  The expected message has no variable parts at all; 'Nonexistent Station' appears only in the emulator's text, where it is the groundStationId the run supplied.

In causes 1, 3, 4 and 6 the expected messages are fixed text in full. In causes 2 and 5 the only variable part is the resource id taken from the request path.

## Steps to Reproduce
Cause 1: ReserveContact a contact, then call DescribeContactVersion(contactId=<that contact>, versionId=<a versionId it does not have, e.g. 99>); read Error.Message (status is 434 either way).

Cause 2: call UpdateContact(contactId=<a well-formed uuid that names no contact>, trackingOverrides=...) and read Error.Message.

Cause 3: call CreateDataflowEndpointGroup(endpointDetails=[{"endpoint": {"name": <name>, "address": {"name": "10.0.0.5", "port": 55888}}}]) with securityDetails omitted; read Error.Message.

Cause 4: CreateDataflowEndpointGroup, DeleteDataflowEndpointGroup it, then GetDataflowEndpointGroup on the same id and read Error.Message.

Cause 5: call DeleteMissionProfile(missionProfileId=<a well-formed uuid that names no mission profile>) and read Error.Message, checking that the requested id appears in it.

Cause 6: call ListAntennas(groundStationId=<a name of 4..97 chars that matches no ground station, e.g. 'Nonexistent Station'>) and read Error.Message.
