Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-connectivity-info-roundtrip
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-connectivity-info-roundtrip greengrassv2-service-role-association
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|UpdateConnectivityInfo|2xx->5xx|$.Version
Issue-Key: greengrass|UpdateConnectivityInfo|2xx->5xx|$headers.content-type
Issue-Key: greengrass|UpdateConnectivityInfo|2xx->5xx|status
Issue-Key: greengrass|GetConnectivityInfo|2xx->5xx|$.ConnectivityInfo
Issue-Key: greengrass|GetConnectivityInfo|2xx->5xx|$headers.content-type
Issue-Key: greengrass|GetConnectivityInfo|2xx->5xx|status
Issue-Key: greengrass|UpdateConnectivityInfo|4xx->5xx|$.Message
Issue-Key: greengrass|UpdateConnectivityInfo|4xx->5xx|$headers.content-type
Issue-Key: greengrass|UpdateConnectivityInfo|4xx->5xx|status
Issue-Key: greengrass|AssociateServiceRoleToAccount|2xx->5xx|$.AssociatedAt
Issue-Key: greengrass|AssociateServiceRoleToAccount|2xx->5xx|$headers.content-type
Issue-Key: greengrass|AssociateServiceRoleToAccount|2xx->5xx|status
Issue-Key: greengrass|GetServiceRoleForAccount|2xx->5xx|$.AssociatedAt
Issue-Key: greengrass|GetServiceRoleForAccount|2xx->5xx|$.RoleArn
Issue-Key: greengrass|GetServiceRoleForAccount|2xx->5xx|$headers.content-type
Issue-Key: greengrass|GetServiceRoleForAccount|2xx->5xx|status
Issue-Key: greengrass|AssociateServiceRoleToAccount|4xx->5xx|$.Message
Issue-Key: greengrass|AssociateServiceRoleToAccount|4xx->5xx|$headers.content-type
Issue-Key: greengrass|AssociateServiceRoleToAccount|4xx->5xx|status
Issue-Key: greengrass|DisassociateServiceRoleFromAccount|2xx->5xx|$.DisassociatedAt
Issue-Key: greengrass|DisassociateServiceRoleFromAccount|2xx->5xx|$headers.content-type
Issue-Key: greengrass|DisassociateServiceRoleFromAccount|2xx->5xx|status
Issue-Key: greengrass|GetServiceRoleForAccount|4xx->5xx|$.Message
Issue-Key: greengrass|GetServiceRoleForAccount|4xx->5xx|$headers.content-type
Issue-Key: greengrass|GetServiceRoleForAccount|4xx->5xx|status
Issue-Commit: 6e0b8b1107b57545eaa8864803a2ad1310139146

# Greengrass connectivity info and service role operations return 500 AmbiguousService before dispatch

> _Note: this issue was identified against the code base at commit `6e0b8b1107b57545eaa8864803a2ad1310139146`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectivityInfo

## Description
This bundle carries 8 causes, all on the connectivity-info and account-service-role surface that the Greengrass endpoint serves (these five operations are modeled on com.amazonaws.greengrass#Greengrass 2017-06-07 and are reached through the same 'greengrass' endpoint as GreengrassV2). Shared fault: the emulator's router keys emulated services by endpoint prefix, and both Greengrass (2017-06-07) and GreengrassV2 (2020-11-30) declare endpointPrefix/arnNamespace/sigv4Name 'greengrass' (Smithy: service_info on both models), so requests are refused before any handler runs with HTTP 500, content-type application/x-amz-json-1.1 and body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'greengrass': ... rejected rather than served by an arbitrary one"}. AWS co-hosts both API versions on greengrass.<region>.amazonaws.com and routes by the restJson1 @http method+uri binding, never by refusing. The open behavioral-corpus issue 20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass.md names the same root cause but carries no wire replay key, so these fixture bindings are net-new.

Cause 1 of 8 - UpdateConnectivityInfo happy path.
AWS: Smithy Greengrass.UpdateConnectivityInfo, PUT /greengrass/things/{ThingName}/connectivityInfo, success code 200, output members Version and Message; modeled errors BadRequestException 400 and InternalServerErrorException 500. Recorded response: 200 application/json with a Version string.
Emulator: 500, content-type application/x-amz-json-1.1, Version absent.
Why: a core device's connectivity endpoints cannot be published, and the client gets no Version token to detect concurrent overwrites.
Fix: the request router / service-registry dispatch layer (no handler is reached), plus the restJson1 error serializer emitting x-amz-json-1.1.
Traces: greengrassv2-connectivity-info-roundtrip#task#1 (also #task#3).

Cause 2 of 8 - GetConnectivityInfo.
AWS: Smithy Greengrass.GetConnectivityInfo, GET /greengrass/things/{ThingName}/connectivityInfo, success 200, output ConnectivityInfo list + Message. Recorded response: 200 with ConnectivityInfo entries carrying Id, HostAddress, PortNumber and Metadata.
Emulator: 500, ConnectivityInfo absent.
Why: client devices resolve their broker endpoints through this call, so discovery is impossible and nothing can read back what was written.
Fix: same dispatch layer; then GetConnectivityInfo's handler/serializer.
Traces: greengrassv2-connectivity-info-roundtrip#task#2 (also #task#4).

Cause 3 of 8 - UpdateConnectivityInfo with a malformed connectivity entry.
AWS: BadRequestException is modeled on UpdateConnectivityInfo with httpError 400. Recorded response: 400 application/json with body member Message (example 'The following required parameter(s) are unparsable: connectivityInfoList.').
Emulator: 500 with the AmbiguousService body; Message absent.
Why: a malformed request is reported as a server fault, so a client retries instead of correcting its input.
Fix: same dispatch layer; then UpdateConnectivityInfo's input validation and its error serializer.
Traces: greengrassv2-connectivity-info-roundtrip#task#5.

Cause 4 of 8 - AssociateServiceRoleToAccount happy path.
AWS: Smithy Greengrass.AssociateServiceRoleToAccount, PUT /greengrass/servicerole, success 200, output AssociatedAt; RoleArn is required in the input. Recorded response: 200 application/json with AssociatedAt present (value not asserted).
Emulator: 500, AssociatedAt absent.
Why: the account service role cannot be attached, which is a prerequisite for deployments, so account setup cannot complete.
Fix: same dispatch layer; then the operation's handler/state.
Traces: greengrassv2-service-role-association#task#2.

Cause 5 of 8 - GetServiceRoleForAccount with a role associated.
AWS: Smithy Greengrass.GetServiceRoleForAccount, GET /greengrass/servicerole, success 200, output RoleArn + AssociatedAt. Recorded response: 200 with the associated RoleArn and AssociatedAt.
Emulator: 500, both members absent.
Why: a client cannot read back which role is attached, so it cannot verify or idempotently re-apply account setup.
Fix: same dispatch layer; then the operation's handler/state.
Traces: greengrassv2-service-role-association#task#3.

Cause 6 of 8 - AssociateServiceRoleToAccount with an invalid role ARN.
AWS: BadRequestException is modeled with httpError 400. Recorded response: 400 application/json with body member Message (example 'That role ARN is invalid.').
Emulator: 500 with the AmbiguousService body; Message absent.
Why: an invalid ARN is reported as a server fault, so misconfiguration looks like an outage.
Fix: same dispatch layer; then the operation's RoleArn validation.
Traces: greengrassv2-service-role-association#task#4.

Cause 7 of 8 - DisassociateServiceRoleFromAccount.
AWS: Smithy Greengrass.DisassociateServiceRoleFromAccount, DELETE /greengrass/servicerole, success 200, output DisassociatedAt. Recorded response: 200 with DisassociatedAt present (value not asserted).
Emulator: 500, DisassociatedAt absent.
Why: the service role can never be detached, so teardown cannot complete.
Fix: same dispatch layer; then the operation's handler/state.
Traces: greengrassv2-service-role-association#task#5.

Cause 8 of 8 - GetServiceRoleForAccount with no role associated.
AWS: recorded behavior - 404 application/json with body member Message (example 'You need to attach an IAM role to this AWS account.'). The modeled error set for this operation is only InternalServerErrorException, so the 404 status is taken from the recording.
Emulator: 500 with the AmbiguousService body; Message absent.
Why: 'no role attached' is the normal pre-setup state and must be distinguishable from a server fault; a client cannot branch on it to attach the role.
Fix: same dispatch layer; then GetServiceRoleForAccount's not-associated path.
Traces: greengrassv2-service-role-association#task#6.

## Evidence
Expected (AWS / recorded): UpdateConnectivityInfo -> 200 with Version; GetConnectivityInfo -> 200 with a ConnectivityInfo list (Id/HostAddress/PortNumber/Metadata); UpdateConnectivityInfo with a malformed entry -> 400 with a Message member; AssociateServiceRoleToAccount -> 200 with AssociatedAt; GetServiceRoleForAccount (associated) -> 200 with RoleArn + AssociatedAt; AssociateServiceRoleToAccount with a bad ARN -> 400 with a Message member; DisassociateServiceRoleFromAccount -> 200 with DisassociatedAt; GetServiceRoleForAccount (none associated) -> 404 with a Message member. All with content-type application/json.

Actual (emulator): all eight answer HTTP 500 with content-type application/x-amz-json-1.1 and body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one"}. Every expected response member (Version, ConnectivityInfo, Message, AssociatedAt, RoleArn, DisassociatedAt) is absent. Replay: 47 mismatched steps, score 6.8%, state_reset_failures 0, so these are genuine response divergences and not state bleed.

Model citation: smithy_query operation UpdateConnectivityInfo/GetConnectivityInfo/AssociateServiceRoleToAccount/GetServiceRoleForAccount/DisassociateServiceRoleFromAccount on services/service-com-amazonaws-greengrass-greengrass -> restJson1, success code 200, BadRequestException httpError 400 on the two mutating ops. service_info shows endpointPrefix/arnNamespace/sigv4Name 'greengrass' on BOTH the greengrass (2017-06-07) and greengrassv2 (2020-11-30) models, which is what the router refuses to disambiguate. AmbiguousService is not a modeled error of either service.

## Steps to Reproduce
1. Boot the emulator from this checkout with its default service set (greengrass and greengrassv2 both claim the 'greengrass' endpoint prefix).
2. Replay fixture greengrassv2-connectivity-info-roundtrip, step task#1 (UpdateConnectivityInfo): expected 200 with a Version member, observed 500 AmbiguousService with content-type application/x-amz-json-1.1.
3. Replay the same fixture's task#2 (GetConnectivityInfo, expected 200 with ConnectivityInfo) and task#5 (UpdateConnectivityInfo with an unparsable connectivityInfoList, expected 400 with a Message member).
4. Replay greengrassv2-service-role-association#task#2 (AssociateServiceRoleToAccount, expected 200 with AssociatedAt), #task#3 (GetServiceRoleForAccount, expected 200 with RoleArn), #task#4 (AssociateServiceRoleToAccount with an invalid role ARN, expected 400), #task#5 (DisassociateServiceRoleFromAccount, expected 200 with DisassociatedAt) and #task#6 (GetServiceRoleForAccount with no role attached, expected 404).
5. Observe every step returns 500 AmbiguousService. All 8 causes must answer with their modeled status/code/shape before this issue is closed.
