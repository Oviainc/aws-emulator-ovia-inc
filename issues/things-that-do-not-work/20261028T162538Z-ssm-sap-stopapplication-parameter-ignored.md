Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 804fb9e39527c7d8ad6b71682b2c9d408f11768a

# StopConnectedEntity never stops the connected DBMS

_Filed: 2026-10-28T16:25:38Z_
_Operation: StopApplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `804fb9e39527c7d8ad6b71682b2c9d408f11768a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `StopApplication$StopConnectedEntity` as an input that ACTS: including it stops the connected DBMS. The handler only (a) re-validates the value against the modelled enum and (b) copies it into the operation record's `Properties`. Nothing in the store is touched for it, so a stop that names `StopConnectedEntity: DBMS` and a stop that omits it leave exactly the same observable state: the application goes STOPPING -> STOPPED either way, and the status every `GetDatabase`/`ListDatabases` reports is derived from the application's own status alone (`database_status_for`), with no input from the switch. The handler's own module doc draws the distinction explicitly: both switches are "recorded as that operation's `Properties`", and only `IncludeEc2InstanceShutdown` "also ACTS". So the DBMS half of the contract is unimplemented, and the parameter is observable only as an echoed property.

## Evidence

Smithy model, `StopApplicationInput$StopConnectedEntity` (smithy_query doc): "<p>Specify the <code>ConnectedEntityType</code>. Accepted type is <code>DBMS</code>.</p> <p>If this parameter is included, the connected DBMS (Database Management System) will be stopped.</p>" — mirrored verbatim in the handler header, services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:65-67: "//! * `StopConnectedEntity` (ConnectedEntityType, one of: DBMS) — Specify the ConnectedEntityType. Accepted type is DBMS. If this parameter is included, the connected DBMS (Database Management System) will be stopped."

Handler doc admitting only the other switch acts — stop_application.rs:8-15: "//! The two optional switches the call carries are recorded as that operation's `Properties`." ... "//! `IncludeEc2InstanceShutdown` is one of those switches and it also ACTS: set to true, the Amazon EC2 instances the registration associated with the application are stopped".

The only two uses of the member — stop_application.rs:197-209 (enum re-check) and stop_application.rs:252-255: "let mut properties = ::std::collections::HashMap::new();" / "if let Some(entity) = input.stop_connected_entity { properties.insert(\"StopConnectedEntity\".to_string(), entity); }". The acting branch below it reads the OTHER member only — stop_application.rs:268: "let instances_to_shut_down = if input.include_ec2_instance_shutdown == Some(true) {".

Database status is a pure function of the application status, with no DBMS dimension — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:3430-3438: "pub fn database_status_for(application_status: &str) -> &'static str { match application_status { APPLICATION_STATUS_ACTIVATED => DATABASE_STATUS_RUNNING, ... APPLICATION_STATUS_STOPPED => DATABASE_STATUS_STOPPED, _ => DATABASE_STATUS_UNKNOWN } }".

Test coverage: tests/all/application_lifecycle.rs:1120-1143 drives a stop WITH `StopConnectedEntity: DBMS` and a stop WITHOUT it and asserts the same settled application status ("STOPPED") for both; tests/all/application_lifecycle.rs:1233 asserts only that the switch is echoed back in the record's `Properties`. tests/all/get_database.rs:687-747 pins the database status following the application for a stop that names NO `StopConnectedEntity` (stated rule: "a database runs as part of its application"), which is the rule the modelled sentence above contradicts.

## Steps to Reproduce

1. POST /register-application with a HANA application that names credentials (e.g. {"ApplicationId":"HDB-TEST","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"sap/hdb/system"}]} and let the registration settle.
2. POST /stop-application with {"ApplicationId":"HDB-TEST"} (no StopConnectedEntity) and let it settle; POST /get-database {"ApplicationId":"HDB-TEST","DatabaseId":"SYSTEMDB"} and note Status=STOPPED.
3. POST /start-application, let it settle, then POST /stop-application with {"ApplicationId":"HDB-TEST","StopConnectedEntity":"DBMS"} and let it settle; POST /get-database again.
4. Compare the two reads: the database reports the same status in both runs, and nothing else in the store differs — the switch changed nothing except the echoed Properties entry of the operation record, although the model says including it stops the connected DBMS.
