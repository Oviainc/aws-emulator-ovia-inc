Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Application store is not scoped to the request's Region, so listings and reads cross Regions

_Filed: 2026-10-24T06:28:49Z_
_Operation: ListApplications | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SSM for SAP registrations are per-account AND per-Region: an application registered in us-east-1 is not visible to a caller in eu-west-1 (the emulator's own refusal text even says "No registered application found for account: X, and application: Y", a Region-scoped statement). The emulator keeps ONE flat `applications: BTreeMap<String, StoredApplication>` with no Region partition, while the Region is read from the SigV4 credential scope only to mint the ARN at registration time.

Consequences a client can observe:
- Register `APP1` signing for us-east-1, then call ListApplications signing for eu-west-1: the emulator returns the application with its `arn:aws:ssm-sap:us-east-1:...` ARN, where AWS returns an empty list.
- GetApplication / StartApplication / StopApplication / DeregisterApplication signed for a different Region likewise resolve (and mutate) an application that does not exist in that Region, instead of being refused.
- RegisterApplication rejects the id with 409 ConflictException in a second Region even though the id is free there.

Other crates in this repo partition state by Region explicitly (e.g. elasticloadbalancing `regions: BTreeMap<String, ...>`, kafka `clusters_per_region`), so the convention exists; this crate does not follow it.

## Evidence

Spec/well-established behavior: ssm-sap is a regional service and its ARNs embed the Region (`arn:aws:ssm-sap:<region>:<account>:application/<id>`, built by src/state.rs `application_arn(region, id)` from `request_region(parts)`); registrations are scoped to an account and Region, which the crate's own `no_registered_application` message states. Handler/state: src/state.rs `ComAmazonawsSsmsapSsmsapState.applications` / `operations` / `tags_by_arn` are keyed by id/ARN only — no Region key; src/ops/list_applications.rs pages over `state.applications.values()` without consulting the request Region (it never calls `request_region`). Tests: every suite signs for a single region (`const REGION: &str = "us-east-1"` in tests/all/application_lifecycle.rs and application_reference_refusals.rs); no test exercises a second Region.

## Steps to Reproduce

1. POST /register-application signed with credential scope `.../us-east-1/ssm-sap/aws4_request` for ApplicationId `APP1`
2. POST /list-applications signed with credential scope `.../eu-west-1/ssm-sap/aws4_request`
3. Observe `Applications` contains APP1 with its us-east-1 ARN (AWS returns an empty list)
4. POST /register-application for `APP1` signed for eu-west-1 and observe 409 ConflictException instead of a successful registration in that Region
