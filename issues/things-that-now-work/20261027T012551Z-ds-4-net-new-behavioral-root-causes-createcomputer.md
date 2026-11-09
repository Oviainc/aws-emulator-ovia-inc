Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.create-computer-authentication-failed-rejected ds.enable-ldaps-without-certificate-blocked ds.enum-wire-values-precede-resource-resolution ds.register-event-topic-absent-directory-rejected
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.create-computer-authentication-failed-rejected|run#20|AssertionError|assert attr_values.get("Description") == marker_value, (
Issue-Key: ds.enable-ldaps-without-certificate-blocked|run#10|AssertionError|assert code == "NoAvailableCertificateException", (
Issue-Key: ds.enum-wire-values-precede-resource-resolution|run#27|AssertionError|assert good_update == "DirectoryDoesNotExistException", (
Issue-Key: ds.register-event-topic-absent-directory-rejected|run#27|AssertionError|assert bad_name_message == expected, (
Issue-Commit: 02e2c56a8a1bc6426078af793fc5f7b555907bec

# ds: 4 net-new behavioral root causes — CreateComputer echoes ComputerAttributes verbatim instead of AWS's canonical AD attributes, EnableLDAPS and DescribeUpdateDirectory answer 501, and @pattern ValidationException messages keep the regex anchors

> _Note: this issue was identified against the code base at commit `02e2c56a8a1bc6426078af793fc5f7b555907bec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputer, EnableLDAPS, DescribeUpdateDirectory, RegisterEventTopic

## Description
Four distinct net-new root causes in the Directory Service (ds) emulator, found by the behavioral suite simparity-behavioral-ga-prod-v1. They are independent and must all be closed before this issue is submitted. None of them is one of the failures the replay report marked ALREADY FILED or OWNED BY an open issue.

=== Cause 1 of 4: CreateComputer echoes ComputerAttributes exactly as the request wrote them, instead of the directory's canonical Active Directory attributes ===

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json) — CreateComputer input ComputerAttributes targets Attributes (list of Attribute{Name,Value}); output is Computer{ComputerId, ComputerName, ComputerAttributes}. The model does not state how the attributes come back, so this is recorded behavior only: measured on real AWS (us-west-2, per the check's own note), a CreateComputer that sets a single attribute under the lowercase LDAP name 'description' answers with ComputerAttributes containing the directory's canonical, AD-generated attribute set keyed by AWS's display-cased names — WindowsSamName, Description, DistinguishedName — where Description carries the value the request supplied under 'description'. In other words the attribute the caller set is echoed under its canonical (Pascal-cased) LDAP attribute name, and AWS adds the attributes the domain itself derives for the new account; the response is not a verbatim replay of the request list.

Emulator: the handler persists input.computer_attributes unchanged and the response is a verbatim echo of it. The reply carries exactly one attribute, named with the request's lowercase spelling — attrs={'description': 'sim2real-marker-<id>'} — and carries no WindowsSamName and no DistinguishedName. ComputerName and ComputerId are correct, and the preceding DirectoryUnavailableException and EntityAlreadyExistsException legs of the same check pass.

Why: a client that reads the created account's attributes the way AWS returns them — indexing ComputerAttributes by 'Description', or taking WindowsSamName / DistinguishedName to drive the domain-join or to build an LDAP query against the new object — gets a key miss against the emulator and concludes the attribute was never applied, or has no SAM name / DN to work with at all. Attribute-name case is the observable contract here: the request's spelling and the response's spelling legitimately differ on AWS.

Fix: CreateComputer's handler and the state record it reads back — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs lines 272-296 (the ComputerRecord construction, whose comment currently asserts "Nothing is added to them") together with ComputerRecord::to_computer in crate::state. Canonicalize a recognized LDAP attribute name to the display casing AWS reports it under, and derive the account's own attributes (SAM name from the computer name, distinguished name from the account's placement in the directory's domain) rather than returning only what the request listed.

Traces: ds.create-computer-authentication-failed-rejected (assertion at check_ds_create_computer_authentication_failed_rejected.py:64).

=== Cause 2 of 4: EnableLDAPS answers 501 NotImplemented, so the no-certificate refusal is never produced ===

AWS: Smithy — EnableLDAPS{DirectoryId (required, pattern ^d-[0-9a-f]{10}$), Type (LDAPSType enum, required)} returns an empty output and models NoAvailableCertificateException, InvalidLDAPSStatusException, DirectoryUnavailableException, DirectoryDoesNotExistException, ClientException, InvalidParameterException, UnsupportedOperationException (all @httpError 400) and ServiceException (500). On an Active Managed Microsoft AD for which ListCertificates reports no certificate, EnableLDAPS(Type='Client') is refused server-side with error code NoAvailableCertificateException at HTTP 400 and LDAPS is left un-enabled (DescribeLDAPSSettings reports no entry whose LDAPSStatus is 'Enabled'); once a certificate is registered with RegisterCertificate(Type='ClientLDAPS') and reaches state Registered, the very same EnableLDAPS call succeeds with an empty 200 body and DescribeLDAPSSettings converges on LDAPSStatus 'Enabled'.

Emulator: every EnableLDAPS call answers HTTP 501 with error code NotImplemented and the scaffold message "operation EnableLDAPS is scaffolded but not implemented in this emulator", so neither the refusal nor the positive control is reachable.

Why: a client cannot discover that it must register a certificate before enabling LDAPS — it gets a server-fault-shaped 501/NotImplemented, which is neither a modeled error nor retryable in a meaningful way, instead of the 400 NoAvailableCertificateException its error handling branches on; and the LDAPS enablement path is wholly unusable.

Fix: implement services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/enable_ldaps.rs: resolve the directory, gate on its stage, require at least one Registered certificate for the requested LDAPSType and otherwise return the modeled NoAvailableCertificateException, and record the LDAPS state that DescribeLDAPSSettings reads. The open issue "Implement ds EnableLDAPS: unimplemented scaffold stub" (20261025T030730Z-ds-enableldaps-stub.md) names the stub file only; it does not state the behavior above, which is what this cause records and what the replayed check grades. The separate check ds.ldaps-enable-disable-server-enforced is the one the report attributed to that stub issue — this cause is grounded in a different check.

Traces: ds.enable-ldaps-without-certificate-blocked (assertion at check_ds_enable_ldaps_without_certificate_blocked.py:38).

=== Cause 3 of 4: DescribeUpdateDirectory answers 501 NotImplemented instead of resolving the DirectoryId ===

AWS: Smithy — DescribeUpdateDirectory{DirectoryId (required, pattern ^d-[0-9a-f]{10}$), UpdateType (enum NETWORK|OS|SIZE, required), RegionName (len 8..32), NextToken} returns UpdateActivities + NextToken and models DirectoryDoesNotExistException, AccessDeniedException, ClientException, InvalidNextTokenException, InvalidParameterException (400) and ServiceException (500). With a legal UpdateType wire value ('OS') and a DirectoryId that satisfies the modeled pattern but names nothing, the request clears request validation and then fails resource resolution with code DirectoryDoesNotExistException at HTTP 400, message "Directory: '<the requested DirectoryId>' does not exist. : RequestId: <the response's RequestId>" — fixed text "Directory: '", "' does not exist. : RequestId: ", with the directory id taken from the request and the request id from the response envelope. Ordering matters and the emulator already gets the first half right: UpdateType='Os' is refused ValidationException with the enum-value-set message before the directory is looked up.

Emulator: DescribeUpdateDirectory is a scaffold stub (src/ops/describe_update_directory.rs line 170 returns Scaffolded::not_implemented) and answers HTTP 501 with code NotImplemented, so the valid-enum leg never reaches resolution. The suite's report does not list this operation among the ALREADY FILED stubs, and no open issue names DescribeUpdateDirectory.

Why: an invalid enum and an absent directory become indistinguishable at the only point where the two still differ — a caller polling a directory update for a directory that was deleted sees a 501 server-side fault rather than the 400 DirectoryDoesNotExistException that tells it the directory is gone, and retry/fallback logic keyed on that error never fires.

Fix: implement services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/describe_update_directory.rs — resolve input.directory_id against crate::state and return the modeled DirectoryDoesNotExistException (via the service's shared not-found diagnostic, the same one AddIpRoutes/CreateComputer use) when it names nothing, otherwise report that directory's update activities for the requested UpdateType.

Traces: ds.enum-wire-values-precede-resource-resolution (assertion at check_ds_enum_wire_values_precede_resource_resolution.py:190).

=== Cause 4 of 4: the @pattern ValidationException message renders the regex with its ^ and $ anchors, which AWS strips ===

AWS: Smithy — RegisterEventTopic.TopicName targets TopicName, @required, @length min 1 max 256, @pattern ^[a-zA-Z0-9_-]+$; RegisterEventTopic models ClientException / EntityDoesNotExistException / InvalidParameterException at 400. A TopicName that violates that pattern (e.g. one containing dots) is refused at HTTP 400 with code ValidationException and the standard constraint message:

  "1 validation error detected: Value '<the rejected value>' at '<member name>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the shape's @pattern WITHOUT its ^ and $ anchors>"

Fixed text of the template: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the rejected value from the request, the member name as the wire names it ('topicName'), and the pattern text from the shape's @pattern trait with the anchoring ^ prefix and $ suffix removed.

Emulator: status 400, code ValidationException, the value position and the member position are all correct, but the pattern position is interpolated verbatim from the trait, so the message ends "...regular expression pattern: ^[a-zA-Z0-9_-]+$" instead of "...regular expression pattern: [a-zA-Z0-9_-]+". The delta is exactly the two anchor characters.

Why: a client that parses the reported pattern out of a ValidationException and feeds it back into its own validator, or that asserts on the refusal text, sees two characters AWS never sends. Because one formatter serves every @pattern-constrained member of this service, this affects all of them (DirectoryId, TopicName, …), not just RegisterEventTopic.TopicName. The identical root cause is already filed for ANOTHER service — "ValidationException @pattern message renders the regex with its ^/$ anchors, which AWS strips" (20261025T135447Z-bcm-dashboards-validationexception-pattern-message-renders-the-regex-with-its.md) — which does NOT cover ds; it is named here only because fixing it in the shared constraint-message formatter would resolve both.

Fix: the @pattern branch of the constraint-violation message formatter used by this service's request-validation layer (the pre-handler validate step of the centralized deserialize → validate → handle → serialize walk) — strip the leading ^ and trailing $ from the trait's pattern before interpolating it, leaving the value and member positions unchanged.

Traces: ds.register-event-topic-absent-directory-rejected (assertion at check_ds_register_event_topic_absent_directory_rejected.py:133; the absent-directory legs of that same check — RegisterEventTopic / DescribeEventTopics / DeregisterEventTopic answering EntityDoesNotExistException with "Directory: '<id>' does not exist. : RequestId: <rid>" — run before it and pass).

## Evidence
Suite simparity-behavioral-ga-prod-v1 — total 16/68 passed (23.5%); ds 14/53 (26%), 39 failed. All other ds / ds__ec2 failures in this run were marked ALREADY FILED (unimplemented DescribeCAEnrollmentPolicy, EnableLDAPS, EC2 CreateInternetGateway) or OWNED by an existing open issue and are skipped; the four below are the net-new ones.

Cause 1 — [mismatch] ds.create-computer-authentication-failed-rejected, check_ds_create_computer_authentication_failed_rejected.py:64
  Call: create_computer(DirectoryId=<active d-...>, ComputerName='sim2real<rand>', Password=<one-time>, ComputerAttributes=[{'Name': 'description', 'Value': '<marker>'}])
  Expected (real AWS, us-west-2): Computer.ComputerAttributes keyed {'WindowsSamName': ..., 'Description': '<marker>', 'DistinguishedName': ...}
  Actual (emulator): attrs={'description': '<marker>'}
  AssertionError: expected Description attribute 'sim2real-marker-d0357020e072', got attrs={'description': 'sim2real-marker-d0357020e072'}
  Earlier legs of the same check PASS: CreateComputer on a non-Active directory -> DirectoryUnavailableException with "Directory: '<id>' does not exist. : RequestId: <rid>"; duplicate name -> EntityAlreadyExistsException; ComputerName and ComputerId echo correctly.

Cause 2 — [mismatch] ds.enable-ldaps-without-certificate-blocked, check_ds_enable_ldaps_without_certificate_blocked.py:38
  Call: enable_ldaps(DirectoryId=<active d-...>, Type='Client') on a directory whose list_certificates returns [].
  Expected: HTTP 400, code NoAvailableCertificateException; LDAPS left not Enabled; after register_certificate(Type='ClientLDAPS') the same call succeeds and DescribeLDAPSSettings reaches 'Enabled'.
  Actual: HTTP 501, code NotImplemented, message "operation EnableLDAPS is scaffolded but not implemented in this emulator".
  AssertionError: expected NoAvailableCertificateException enabling LDAPS with no registered certificate, got NotImplemented

Cause 3 — [mismatch] ds.enum-wire-values-precede-resource-resolution, check_ds_enum_wire_values_precede_resource_resolution.py:190
  Call: describe_update_directory(DirectoryId='d-4b7c97286b' (pattern-valid, names nothing), UpdateType='OS')
  Expected: HTTP 400, code DirectoryDoesNotExistException, message "Directory: 'd-4b7c97286b' does not exist. : RequestId: <rid>"
  Actual: HTTP 501, code NotImplemented
  AssertionError: DescribeUpdateDirectory(UpdateType='OS') on absent d-4b7c97286b must fail resolution with DirectoryDoesNotExistException, got NotImplemented
  The preceding legs PASS: DescribeLDAPSSettings(Type='CLIENT') / DescribeClientAuthenticationSettings(Type='SMART_CARD') / DescribeUpdateDirectory(UpdateType='Os') are each refused ValidationException with the enum-value-set message before resolution, and the valid-wire-value legs of the first two return DirectoryDoesNotExistException correctly — so only DescribeUpdateDirectory's resolution path is missing.

Cause 4 — [mismatch] ds.register-event-topic-absent-directory-rejected, check_ds_register_event_topic_absent_directory_rejected.py:133
  Call: register_event_topic(DirectoryId=<absent d-...>, TopicName='sim2real.behav.topic')  (dots violate TopicName's @pattern ^[a-zA-Z0-9_-]+$)
  Code ValidationException and HTTP 400 — MATCHED. Value position and member position ('topicName') — MATCHED.
  Expected message: 1 validation error detected: Value 'sim2real.behav.topic' at 'topicName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+
  Actual message:   1 validation error detected: Value 'sim2real.behav.topic' at 'topicName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_-]+$
  Delta: exactly the leading ^ and trailing $.

Model corroboration (smithy_query query=operation on com-amazonaws-directoryservice-directoryservice-20150416.json): CreateComputer output Computer, errors include DirectoryUnavailableException/EntityAlreadyExistsException; EnableLDAPS errors include NoAvailableCertificateException (client, 400); DescribeUpdateDirectory errors include DirectoryDoesNotExistException (client, 400); RegisterEventTopic.TopicName length 1..256, pattern ^[a-zA-Z0-9_-]+$. Scaffold state confirmed in the checkout: src/ops/describe_update_directory.rs:4 carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and line 170 returns Scaffolded::not_implemented("DescribeUpdateDirectory").

## Steps to Reproduce
Boot the emulator on fresh state and point a botocore client at the ds endpoint (plus ec2 for the VPC fixture, sns for the topic fixture).

Cause 1 (CreateComputer attribute echo):
1. create_vpc + two subnets in distinct AZs; create_microsoft_ad(Name='<unique>.example.com', Password=<strong>, Edition='Standard', VpcSettings=...).
2. Wait until describe_directories reports Stage 'Active'.
3. create_computer(DirectoryId=<id>, ComputerName='sim2real12345', Password='Sim2Real-Computer-Pw1!', ComputerAttributes=[{'Name': 'description', 'Value': 'marker-1'}]).
4. Read resp['Computer']['ComputerAttributes'] as {a['Name']: a['Value']}. Expect a 'Description' key carrying 'marker-1' plus 'WindowsSamName' and 'DistinguishedName'; observe only {'description': 'marker-1'}.

Cause 2 (EnableLDAPS):
1. Same Active directory. list_certificates(DirectoryId=<id>) -> CertificatesInfo == [].
2. enable_ldaps(DirectoryId=<id>, Type='Client'). Expect ClientError code NoAvailableCertificateException at HTTP 400; observe HTTP 501 NotImplemented.
3. describe_ldaps_settings(DirectoryId=<id>) must show no entry with LDAPSStatus 'Enabled'.
4. Positive control once implemented: register_certificate(DirectoryId=<id>, CertificateData=<self-signed PEM>, Type='ClientLDAPS'), wait for state Registered, then the same enable_ldaps call must succeed and DescribeLDAPSSettings must reach 'Enabled'.

Cause 3 (DescribeUpdateDirectory):
1. absent = 'd-' + 10 hex chars that name no directory.
2. describe_update_directory(DirectoryId=absent, UpdateType='Os') -> expect ValidationException with "...Member must satisfy enum value set: [SIZE, OS, NETWORK]" (passes today).
3. describe_update_directory(DirectoryId=absent, UpdateType='OS') -> expect code DirectoryDoesNotExistException, HTTP 400, message "Directory: '<absent>' does not exist. : RequestId: <the response RequestId>"; observe HTTP 501 NotImplemented.

Cause 4 (@pattern message anchors):
1. sns.create_topic(Name='sim2real-behav-<unique>'); absent = 'd-' + 10 hex chars.
2. register_event_topic(DirectoryId=absent, TopicName='sim2real.behav.topic').
3. Read err.response['Error']['Message']: expect it to end "...regular expression pattern: [a-zA-Z0-9_-]+"; observe "...regular expression pattern: ^[a-zA-Z0-9_-]+$".
4. Re-check any other @pattern-constrained ds member once the formatter is fixed (e.g. a malformed DirectoryId on DescribeLDAPSSettings).

Re-replay to confirm: ds.create-computer-authentication-failed-rejected, ds.enable-ldaps-without-certificate-blocked, ds.enum-wire-values-precede-resource-resolution, ds.register-event-topic-absent-directory-rejected
