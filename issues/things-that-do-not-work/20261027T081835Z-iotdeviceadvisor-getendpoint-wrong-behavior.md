Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# GetEndpoint vends a device-level host that disagrees with the endpoint StartSuiteRun reports for the same device

_Filed: 2026-10-27T08:18:35Z_
_Operation: GetEndpoint | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The GetEndpoint handler's own module doc promises that the host it vends and the host a suite run carries cannot disagree, because both come from one derivation that `StartSuiteRun` also reports from. The code has TWO derivations and uses the one `StartSuiteRun` never calls: when the request names a device (thingArn / certificateArn / deviceRoleArn) GetEndpoint answers `crate::state::endpoint_for_device(...)`, a SHA-256-derived per-device host, while `StartSuiteRun` always stores and reports `crate::state::endpoint_for_region(...)`, the account-level host — even though the run's `suiteRunConfiguration.primaryDevice` names exactly such a device.

So for the device a run was started for, the two operations report two different hosts: a client that starts a run for thing T (and is told endpoint E1) and then asks `GET /endpoint?thingArn=T` where that device should connect is handed a different host E2. The crate's own tests prove the two values differ (a device-level endpoint is asserted to differ from the account-level one), and the test that pins the "one derivation" promise only ever calls GetEndpoint with NO device query, so the device path is unchecked. The same false claim is restated on `endpoint_for_region` in the state module.

## Evidence

The promise, in the handler's own doc — services/.../src/ops/get_endpoint.rs:5-11:
  "//! Reports the Device Advisor host a device under test connects to: the ACCOUNT-LEVEL
   //! host for a request that names no device, and the DEVICE-LEVEL host for one that names
   //! a device ... the host is derived from the account, the request's signing region and the device named, by the
   //! single derivation in [`crate::state`] that `StartSuiteRun` already reports from, so
   //! nothing is written and the endpoint a run carries and the one this vends cannot
   //! disagree."

The code under that doc takes a SECOND derivation — services/.../src/ops/get_endpoint.rs:227-231:
  "let endpoint = if device.names_no_device() {
       crate::state::endpoint_for_region(&region)
   } else {
       crate::state::endpoint_for_device(&region, &device)
   };"

What StartSuiteRun actually stores and reports — services/.../src/ops/start_suite_run.rs:286 (inside the `SuiteRun` it persists, whose `configuration` carries `primaryDevice`):
  "endpoint: crate::state::endpoint_for_region(&region),"

The state module repeats the same claim — services/.../src/state.rs:913-917:
  "/// Device Advisor vends one account-scoped host of the form
   /// `<account>.deviceadvisor.iot.<region>.amazonaws.com`, which is what a suite run reports
   /// and what a device under test connects to. It is derived here rather than stored, so
   /// every operation that has to report it — `StartSuiteRun`, the suite-run reads, and
   /// `GetEndpoint` — answers the same host for the same account and region."
while the second derivation lives at services/.../src/state.rs:1147: "pub fn endpoint_for_device(region: &str, device: &DeviceIdentity<'_>) -> String".

The crate's tests prove the two hosts differ, and that the agreement is only checked for the no-device path:
* tests/all/endpoint.rs:438-441: "assert_ne!(endpoint, account_level, \"case {case:?}: a device-level endpoint must differ from the account-level one\");"
* tests/all/endpoint.rs:260-261 + 306-316: "/// The host `GetEndpoint` vends is the host a suite run of that region PERSISTED: one\n/// derivation, so what a device is told to connect to cannot differ between the two." — yet the run is started with "primaryDevice: { thingArn: thing_arn(\"endpoint-device\") }" (line 294-297) and the comparison is made against "emulator.get_endpoint(&[], region)" (line 306), a request that names NO device.

## Steps to Reproduce

1. POST /suiteDefinitions with a valid suiteDefinitionConfiguration (rootGroup "g", devicePermissionRoleArn an IAM role ARN); keep suiteDefinitionId.
2. POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns with {"suiteRunConfiguration":{"primaryDevice":{"thingArn":"arn:aws:iot:us-east-1:000000000000:thing/T"}}} and note the `endpoint` in the response (E1 = <account>.deviceadvisor.iot.us-east-1.amazonaws.com, from endpoint_for_region).
3. GET /endpoint?thingArn=arn%3Aaws%3Aiot%3Aus-east-1%3A000000000000%3Athing%2FT and note the `endpoint` (E2, a 14-character digest label, from endpoint_for_device).
4. Observe E2 != E1 — the device named by the run is told one host by StartSuiteRun and another by GetEndpoint, although get_endpoint.rs:5-11 states the two "cannot disagree" and state.rs:913-917 states every operation reporting the endpoint "answers the same host".
