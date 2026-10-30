Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-catalog-list-configuration-check-definitions
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-catalog-list-configuration-check-definitions ssm-sap-configuration-check-catalog ssm-sap-list-components-nonexistent-application-not-found
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP EBS Storage Configuration"]
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP EC2 Instance Type Selection"]
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP HANA database connectivity"]
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP application component layout"]
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP on AWS Pacemaker Configuration"]
Issue-Key: ssm-sap|ListConfigurationCheckDefinitions|2xx->2xx|$.ConfigurationChecks[Name="SAP system identification"]
Issue-Key: ssm-sap|ListComponents|2xx->2xx|$.Components[*]
Issue-Commit: 9aa3e2b9dc38edbf8faf15afd04e4b717b6bfa72

# ssm-sap read operations serve a fabricated inventory: configuration-check catalog entries and component summaries the service never reports

> _Note: this issue was identified against the code base at commit `9aa3e2b9dc38edbf8faf15afd04e4b717b6bfa72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConfigurationCheckDefinitions

## Description
Three root causes where an ssm-sap listing answers 200 with content the service does not serve: the configuration-check catalog is populated with the emulator's own invented check names and descriptions, and a component listing reports a component summary that the recorded service does not list.

Related open issue (does NOT cover these wire steps; it is grounded in behavioral check ids and carries no replay key for this corpus): 20261029T184631Z-ssm-sap-registration-state-machine-fidelity-discovery-always.md, which describes fabricated components from a different angle. Cause 3 below must still be closed against the fixture step named here.

Cause 1 of 3: ListConfigurationCheckDefinitions returns the wrong catalog entries.
AWS: the configuration-check catalog is a FIXED, service-owned list keyed by the modeled check ids, and every entry's Name and Description are the service's own published text, identical in every account and Region. The recorded catalog is: SAP_CHECK_01 = Name 'SAP EC2 Instance Type Selection', Description 'Checks any EC2 Instances Associated with this SAP Application, and evaluates whether the Instance Type complies with SAP Certification requirements and that necessary hardware settings are in place.'; SAP_CHECK_02 = Name 'SAP EBS Storage Configuration', Description 'Checks that the EBS Storage Configuration on Amazon EC2 Instances is appropriate for the SAP workload.'; SAP_CHECK_03 = Name 'SAP on AWS Pacemaker Configuration', Description 'Checks whether the Pacemaker Configuration for the SAP Application follows best practices.'; each with ApplicableApplicationTypes ['HANA','SAP_ABAP']. Smithy: ConfigurationCheckType enum values = [SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03]; ConfigurationCheckDefinition members Id/Name/Description/ApplicableApplicationTypes; ListConfigurationCheckDefinitions is readonly, paginated over ConfigurationChecks. The Name/Description text is recorded behavior only — the model documents the members, not their values.
Emulator: the ids and ApplicableApplicationTypes match, but all three Names and Descriptions are the emulator's own prose — SAP_CHECK_01 'SAP system identification' ('Checks that the application identifies the SAP system it manages and the Amazon EC2 instances that system runs on.'), SAP_CHECK_02 'SAP HANA database connectivity' ('Checks the SAP HANA databases of the application and the credentials the service connects to them with.'), SAP_CHECK_03 'SAP application component layout' ('Checks the components the application declares for its SAP system and where they run.'). Matched by Name, every expected entry is missing and every served entry is extra.
Why: the catalog is what an operator or a console/CLI workflow reads to pick a check before StartConfigurationChecks, and these Names are stable service-published strings, not per-account data — any client that selects, displays, filters or asserts on a check by Name finds none of the real checks and three checks that do not exist on AWS.
Fix: the static configuration-check catalog table that backs the ListConfigurationCheckDefinitions handler (and whatever GetConfigurationCheckOperation/StartConfigurationChecks read from the same table) must carry the service's published Name/Description per check id.
Traces: ssm-sap-catalog-list-configuration-check-definitions#task#1.

Cause 2 of 3: the same catalog divergence on the second catalog fixture, which asserts only the three expected entries.
AWS: as Cause 1 — SAP_CHECK_01/02/03 with the published Names and Descriptions above.
Emulator: the three expected definitions are reported missing from the 200 response, confirming the defect is in the catalog data itself and not in how one fixture matched the list (this cluster has its own fixture step, so it is its own replay unit).
Why: as Cause 1.
Fix: as Cause 1 — one catalog table; fixing it closes this cluster too, but it must be re-replayed to confirm.
Traces: ssm-sap-configuration-check-catalog#task#1.

Cause 3 of 3: ListComponents reports a component the service does not list for that application.
AWS: a component appears in ListComponents only once the service has actually discovered it for a registered application; the listing is not derived from the ComponentsInfo a registration declared. In the recorded step the 200 response carries NO component for the application under test (an empty Components list), and the handler's own contract is that an ApplicationId naming no registered application is refused with ResourceNotFoundException (HTTP 404) rather than answered with another application's inventory. Smithy: ListComponents is readonly and paginated over Components (ComponentSummary: ApplicationId, ComponentId, ComponentType, Tags, Arn), with ResourceNotFoundException (httpError 404), UnauthorizedException (401) and ValidationException (400) modeled. The emptiness of the recorded listing is recorded behavior only.
Emulator: the 200 response carries an extra ComponentSummary — ComponentId 'HDB-HANA', ComponentType 'HANA', with an ApplicationId and a component ARN of the form arn:aws:ssm-sap:<region>:<account>:component/<application-id>/<component-id> — for an application the recording lists no components for. Hypothesis: the handler projects components straight out of the registration's declared ComponentsInfo (see the candidate build in the ListComponents handler), so a component exists the instant RegisterApplication returns and survives conditions under which the real service lists none.
Why: a client that waits for discovery by polling ListComponents until it is non-empty never observes the pending state, and a client that uses the listing to decide what exists acts on a component that AWS would not report — including, after the owning application is gone, a component whose ARN resolves to nothing.
Fix: the component inventory in the ListComponents handler and the service state it reads — components must be produced by the discovery path and dropped with their application, not fabricated from the registration input.
Traces: ssm-sap-list-components-nonexistent-application-not-found#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up), per cause.

Cause 1 — ssm-sap-catalog-list-configuration-check-definitions#task#1, ListConfigurationCheckDefinitions, status 200 expected and 200 observed; six keys diverge under $.ConfigurationChecks matched by Name:
  missing: Name 'SAP EC2 Instance Type Selection' (Id SAP_CHECK_01, Description 'Checks any EC2 Instances Associated with this SAP Application, and evaluates whether the Instance Type complies with SAP Certification requirements and that necessary hardware settings are in place.', ApplicableApplicationTypes ['HANA','SAP_ABAP'])
  missing: Name 'SAP EBS Storage Configuration' (Id SAP_CHECK_02, Description 'Checks that the EBS Storage Configuration on Amazon EC2 Instances is appropriate for the SAP workload.')
  missing: Name 'SAP on AWS Pacemaker Configuration' (Id SAP_CHECK_03, Description 'Checks whether the Pacemaker Configuration for the SAP Application follows best practices.')
  extra:   Name 'SAP system identification' (Id SAP_CHECK_01), Name 'SAP HANA database connectivity' (Id SAP_CHECK_02), Name 'SAP application component layout' (Id SAP_CHECK_03) — the emulator's own text.

Cause 2 — ssm-sap-configuration-check-catalog#task#1, ListConfigurationCheckDefinitions, status 200 expected and 200 observed:
  the same three definitions (SAP EC2 Instance Type Selection / SAP EBS Storage Configuration / SAP on AWS Pacemaker Configuration) are reported missing from the response.

Cause 3 — ssm-sap-list-components-nonexistent-application-not-found#task#4, ListComponents, status 200 expected and 200 observed:
  $.Components[0]: extra (expected None, got {'ApplicationId': '<application-id>', 'ComponentId': 'HDB-HANA', 'ComponentType': 'HANA', 'Arn': 'arn:aws:ssm-sap:<region>:<account>:component/<application-id>/HDB-HANA'}). The recorded run's application id (sim2real-sap-a1462b72-530b-41be-87b3-732f7f778e3d), account and Region are per-run values; the divergence is that any component is listed at all.

## Steps to Reproduce
Causes 1-2: POST /list-configuration-check-definitions {} -> observe 200 with ConfigurationChecks naming 'SAP system identification' / 'SAP HANA database connectivity' / 'SAP application component layout'; AWS returns 'SAP EC2 Instance Type Selection' (SAP_CHECK_01), 'SAP EBS Storage Configuration' (SAP_CHECK_02) and 'SAP on AWS Pacemaker Configuration' (SAP_CHECK_03) with the published descriptions quoted in Evidence. Re-replay fixtures ssm-sap-catalog-list-configuration-check-definitions step task#1 and ssm-sap-configuration-check-catalog step task#1.

Cause 3: replay fixture ssm-sap-list-components-nonexistent-application-not-found up to step task#4 — POST /register-application for the probe application with ComponentsInfo, then the step's POST /list-components -> observe a 200 carrying a ComponentSummary with ComponentId 'HDB-HANA'; the recording's 200 carries no components for that application. Confirm with the same fixture step after the fix.
