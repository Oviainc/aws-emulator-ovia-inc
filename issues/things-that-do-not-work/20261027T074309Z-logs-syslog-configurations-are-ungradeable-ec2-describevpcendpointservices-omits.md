Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.syslog-configuration-delete-nonexistent-rejected logs.syslog-configuration-lifecycle logs.syslog-configuration-lifecycle-transitions
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.syslog-configuration-delete-nonexistent-rejected|_find_syslog_vpc_endpoint_service_name#6|AssertionError|raise AssertionError(
Issue-Key: logs.syslog-configuration-lifecycle|_find_syslog_vpc_endpoint_service_name#5|AssertionError|raise AssertionError(
Issue-Key: logs.syslog-configuration-lifecycle-transitions|_find_syslog_service_name#4|AssertionError|raise AssertionError(
Issue-Commit: 5df675e0ea220f5d7ca93176f138364900f5089e

# logs syslog configurations are ungradeable: EC2 DescribeVpcEndpointServices omits the CloudWatch Logs syslog-ingestion PrivateLink service, so no syslog VPC endpoint can be created

> _Note: this issue was identified against the code base at commit `5df675e0ea220f5d7ca93176f138364900f5089e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSyslogConfiguration

## Description
One net-new root cause in the CloudWatch Logs (`logs`) surface, grounded in three failing behavioral checks whose ids lead with `logs.`. None of the three is marked ALREADY FILED or OWNED in the replay report. Cause count: 1 — it must be closed before this issue is submitted.

RELATED OPEN ISSUE (it does NOT cover this cause and does not carry any of these three check ids in its Issue-Trace-Ids): 20261026T063806Z-logs-cloudwatch-logs-createdelivery-refuses-an-idempotent-re-pair.md. Its cause 2 is about an EC2 interface VPC endpoint that is successfully CREATED (against a hard-coded ServiceName) and then never reports State "available" in DescribeVpcEndpoints. The cause below is one step EARLIER and in a different operation: the account/Region endpoint-service CATALOG that DescribeVpcEndpointServices publishes contains no syslog-ingestion service at all, so these three checks never get as far as CreateVpcEndpoint. Fixing the endpoint state machine does not add a catalog entry, and adding a catalog entry does not advance an endpoint to "available"; both are needed before the logs syslog surface is reachable. The stub issue 20261022T235108Z-ec2-describevpcendpointservices-stub.md is marked fixed and is NOT being reopened here: DescribeVpcEndpointServices now answers HTTP 200 normally — the divergence is the CONTENT of that 200, not a refusal.

=== Cause 1 of 1: the EC2 endpoint-service catalog publishes no dedicated CloudWatch Logs syslog-ingestion interface endpoint service ===
AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) models the syslog surface as PutSyslogConfiguration(logGroupIdentifier @required @length 1..2048 @pattern ^[\w#+=/:,.@-]*$, vpcEndpointId @pattern ^vpce-[0-9a-f]{1,64}$) -> empty body; ListSyslogConfigurations(logGroupIdentifier, vpcEndpointId, nextToken, maxResults @range 0..50) -> {syslogConfigurations, nextToken}; DeleteSyslogConfiguration(logGroupIdentifier @required, vpcEndpointId) -> empty body. Each models AccessDeniedException, InvalidOperationException, InvalidParameterException, ResourceNotFoundException, ThrottlingException (HTTP 400) and ServiceUnavailableException (500); Put/Delete additionally model OperationAbortedException (400). The `vpcEndpointId` these operations take is not any interface endpoint: it must be an endpoint to the DEDICATED regional syslog-ingestion PrivateLink service, which on real AWS is published as `com.amazonaws.<region>.syslog-logs` and is NOT the plain CloudWatch Logs API service `com.amazonaws.<region>.logs` (the checks state in-line that an endpoint to the plain logs API service is rejected by PutSyslogConfiguration, and that a guessed name such as `com.amazonaws.<region>.logs-syslog` is refused with InvalidServiceName). Recorded behavior only for the catalog itself: ec2 DescribeVpcEndpointServices, paged with Filters=[{Name:'service-type',Values:['Interface']}], returns a ServiceDetails list that CONTAINS a syslog-ingestion entry for the request's Region alongside `com.amazonaws.<region>.logs` — the regional name is derived from the request's Region and is never a literal from one run.
Emulator: DescribeVpcEndpointServices answers HTTP 200, but for the signing Region the only logs-related entry it publishes is `com.amazonaws.<region>.logs` (this run, us-west-2: ServiceNames/ServiceDetails contained exactly `com.amazonaws.us-west-2.logs` and no name containing "syslog"). Discovery therefore finds zero candidates and all three checks abort in their own provisioning helper before any `logs` syslog operation is issued: `AssertionError: no dedicated syslog-ingestion VPC endpoint service is available in this account/region for CloudWatch Logs (logs-related endpoint services seen: ['com.amazonaws.us-west-2.logs'])` and `AssertionError: no CloudWatch Logs syslog-ingestion VPC endpoint service is available in <region>`. Because the catalog is the gate, PutSyslogConfiguration, ListSyslogConfigurations and DeleteSyslogConfiguration are never called at all, so the emulator is never asked for any of their contracts.
Why: a client that provisions CloudWatch Logs syslog ingestion starts by discovering the regional syslog PrivateLink service name — DescribeVpcEndpointServices is the documented and only supported way to obtain it, precisely because the name cannot be guessed. Against AWS the discovery returns a name and the endpoint can be created; against the emulator the discovery returns nothing, so CreateVpcEndpoint can only be called with a name the catalog does not know (which DescribeVpcEndpointServices itself refuses with InvalidServiceName via its own `ServiceNames` check), and the whole feature is unreachable. Nothing about the syslog contract can be observed or regression-tested: that a Put becomes visible in ListSyslogConfigurations with its vpcEndpointId and the log group's logGroupArn; that a Delete removes exactly the configuration for that (logGroupIdentifier, vpcEndpointId) pair; and that a Delete for a pair with no configuration — never created OR just deleted — is refused HTTP 400 InvalidParameterException whose message's fixed text is "VPC endpoint " … " is not associated with log group " with the two variable parts taken from the request's vpcEndpointId and logGroupIdentifier, while the log group itself survives untouched.
Fix: the EC2 Region endpoint-service catalog — `crate::state::endpoint_services_in_region` in services/service-com-amazonaws-ec2-amazonec2/src/state.rs, which services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs:290-293 uses as the sole source of AWS-offered services for a Region — must include the CloudWatch Logs syslog-ingestion Interface service for the Region, named as AWS publishes it (`com.amazonaws.<region>.syslog-logs`) and distinct from `com.amazonaws.<region>.logs`, so that the `ServiceNames` existence check at describe_vpc_endpoint_services.rs:318-327 and CreateVpcEndpoint both accept it. Once the catalog entry exists and the endpoint reaches a usable state, the PutSyslogConfiguration / ListSyslogConfigurations / DeleteSyslogConfiguration handlers in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/ must key the configuration on (logGroupIdentifier, vpcEndpointId), reject an endpoint that is not an endpoint to the syslog-ingestion service, and raise the InvalidParameterException "not associated" refusal above for an absent pair.
Traces: logs.syslog-configuration-delete-nonexistent-rejected, logs.syslog-configuration-lifecycle, logs.syslog-configuration-lifecycle-transitions.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (150/428 passed; service `logs` 134/243). All three checks are listed under `### logs` with no OWNED/ALREADY-FILED marker.

[mismatch] logs.syslog-configuration-delete-nonexistent-rejected
  Error: File "check_logs_syslog_configuration_delete_nonexistent_rejected.py", line 92, in _create_syslog_vpc_endpoint
    service_name = _find_syslog_vpc_endpoint_service_name(ec2)
  File "check_logs_syslog_configuration_delete_nonexistent_rejected.py", line 134, in _find_syslog_vpc_endpoint_service_name
    AssertionError: no dedicated syslog-ingestion VPC endpoint service is available in this account/region for CloudWatch Logs (logs-related endpoint services seen: ['com.amazonaws.us-west-2.logs'])
  Reached after logs CreateLogGroup succeeded, then ec2 CreateVpc(CidrBlock='10.212.0.0/16'), CreateSubnet(CidrBlock='10.212.1.0/24') and CreateSecurityGroup all returned without raising. The discovery helper pages ec2 DescribeVpcEndpointServices with Filters=[{'Name':'service-type','Values':['Interface']}], collects every ServiceDetails[].ServiceName containing "logs", and selects the first whose name also contains "syslog".
  Expected: at least one ServiceName containing both "logs" and "syslog" for the request's Region (AWS: com.amazonaws.<region>.syslog-logs), so CreateVpcEndpoint can be issued against it.
  Actual: the only logs-related name in the whole paged catalog is com.amazonaws.us-west-2.logs; zero syslog candidates.
  Left unexercised by this abort: logs ListSyslogConfigurations(logGroupIdentifier=G) == [] baseline; DeleteSyslogConfiguration(logGroupIdentifier=G, vpcEndpointId=E) for a never-configured pair refused 400 InvalidParameterException with the message "VPC endpoint <E> is not associated with log group <G>"; the group still present in DescribeLogGroups(logGroupNamePrefix=G) after the rejected delete; the positive control Put -> list of exactly 1 -> Delete -> empty list; and the SECOND delete of the just-deleted pair refused with the same code and the same message.

[mismatch] logs.syslog-configuration-lifecycle
  Error: File "check_logs_syslog_configuration_lifecycle.py", line 162, in _create_logs_vpc_endpoint
    service_name = _find_syslog_vpc_endpoint_service_name(ec2)
  File "check_logs_syslog_configuration_lifecycle.py", line 199, in _find_syslog_vpc_endpoint_service_name
    AssertionError (same discovery assertion as above).

[mismatch] logs.syslog-configuration-lifecycle-transitions
  Error: File "check_logs_syslog_configuration_lifecycle_transitions.py", line 187, in _create_vpc_endpoint
    service_name = _find_syslog_service_name(ec2, region)
  File "check_logs_syslog_configuration_lifecycle_transitions.py", line 171, in _find_syslog_service_name
    AssertionError: no CloudWatch Logs syslog-ingestion VPC endpoint service is available in us-west-2
  Its helper documents the requirement in-line: "Discover the syslog-ingestion interface-endpoint service name for this region. A guessed \"com.amazonaws.<region>.logs-syslog\" is rejected with InvalidServiceName." — i.e. the name must come from the catalog, it cannot be synthesized by the client.

Emulator source confirming the gate: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs builds its candidate set only from `state::endpoint_services_in_region(region)` plus the account's own stored `vpc_endpoint_services` configurations (lines 286-303), and refuses any requested name absent from that set with InvalidServiceName (lines 318-327). No syslog-ingestion entry is derived for the Region, so neither discovery nor an explicitly named CreateVpcEndpoint can reach it.

(The Region `us-west-2`, the name `com.amazonaws.us-west-2.logs`, the CIDRs and the generated group/endpoint ids above are this run's values filling the templates; the requirement is the presence of a Region-derived syslog-ingestion service name distinct from the plain logs API service.)

## Steps to Reproduce
Boot the emulator, then:
  1. ec2 DescribeVpcEndpointServices, paged to exhaustion, with Filters=[{'Name':'service-type','Values':['Interface']}].
     -> OBSERVE the only ServiceDetails[].ServiceName containing "logs" is `com.amazonaws.<region>.logs`; no name contains "syslog".
     -> EXPECT a dedicated CloudWatch Logs syslog-ingestion Interface service for the request's Region (AWS: `com.amazonaws.<region>.syslog-logs`) listed alongside it, in both ServiceNames and ServiceDetails.
  2. ec2 DescribeVpcEndpointServices(ServiceNames=['com.amazonaws.<region>.syslog-logs'])
     -> OBSERVE InvalidServiceName; EXPECT HTTP 200 describing that service.
  Then, once the catalog entry exists, confirm the gated surface:
  3. logs CreateLogGroup(logGroupName=G)
  4. ec2 CreateVpc(CidrBlock='10.212.0.0/16'); CreateSubnet(VpcId=<vpc>, CidrBlock='10.212.1.0/24'); CreateSecurityGroup(VpcId=<vpc>, GroupName=<sg>, Description=...)
  5. ec2 CreateVpcEndpoint(VpcId=<vpc>, ServiceName=<the discovered syslog service name>, VpcEndpointType='Interface', SubnetIds=[<subnet>], SecurityGroupIds=[<sg>], PrivateDnsEnabled=False) -> note VpcEndpointId as E; poll ec2 DescribeVpcEndpoints(VpcEndpointIds=[E]) until State == 'available'.
  6. logs ListSyslogConfigurations(logGroupIdentifier=G) -> expect syslogConfigurations == [].
  7. logs DeleteSyslogConfiguration(logGroupIdentifier=G, vpcEndpointId=E) on the never-configured pair -> expect HTTP 400 InvalidParameterException, Message "VPC endpoint <E> is not associated with log group <G>" (fixed text "VPC endpoint " / " is not associated with log group "; the two values come from the request). Then logs DescribeLogGroups(logGroupNamePrefix=G) still lists G, and ListSyslogConfigurations(logGroupIdentifier=G) is still [].
  8. logs PutSyslogConfiguration(logGroupIdentifier=G, vpcEndpointId=E) -> expect 200; ListSyslogConfigurations(logGroupIdentifier=G) -> expect exactly one entry, vpcEndpointId == E and logGroupArn == G's arn.
  9. logs DeleteSyslogConfiguration(logGroupIdentifier=G, vpcEndpointId=E) -> expect 200; ListSyslogConfigurations(logGroupIdentifier=G) -> expect []. Repeat the delete -> expect the same 400 InvalidParameterException and the same message as step 7.
