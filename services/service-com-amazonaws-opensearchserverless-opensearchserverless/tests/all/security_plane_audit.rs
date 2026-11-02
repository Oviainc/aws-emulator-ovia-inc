// SPDX-License-Identifier: CC0-1.0

//! The audit record every **security policy** and **security configuration** call leaves,
//! end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body) through the
//! harness this crate's `collection_lifecycle` suite owns, so the boundary validator, the
//! protocol codec and the handler all take part exactly as they do for an SDK client. The
//! records are read back from the one `tracing` capture `audit_capture` installs.
//!
//! The rule under test: every call this service's policy plane serves leaves exactly ONE
//! structured record naming the service, the operation, the identity the request presented,
//! the request target, the resource the call resolved — none for a listing, which addresses
//! a whole type rather than one resource — and the outcome it answered; the READ of a
//! resource as much as the write of it, and whatever the call answered.
//!
//! The table spans what the rule generalises over: both halves of the plane (policies
//! addressed by `(type, name)` and configurations addressed by their id), every operation
//! on each (create, update, delete, get, list), each resource type, the accepted call and
//! every refusal a handler takes, the outcome each exit records, and the Region the call is
//! signed for. A request the framework's own constraint walk refuses — a value outside an
//! enum's value list among them — never reaches a handler, so no row drives one: the record
//! is owed by the call the service served.

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
    MAX_SECURITY_CONFIGS_PER_TYPE, MAX_SECURITY_POLICIES_PER_TYPE,
};

use crate::audit_capture::{audit_capture, CapturedRecords};
use crate::collection_lifecycle::{call_signed_as, emulator, text, ACCOUNT, CLIENT_ERROR};
use crate::security_configs::{saml_options, INSTANCE_ARN};

/// This service's SigV4 signing name, which every audit record names as its service
/// (`smithy_query(query="service_info")` -> `sigv4Name`).
const SERVICE: &str = "aoss";

/// The access key id THIS suite signs with, presented in the `Authorization` header's
/// credential scope and reported by `emulator_server::extract_akid`.
///
/// Its own, used by no sibling suite: the capture is this test binary's single global
/// subscriber, so the records of a suite running in parallel over the same routes are told
/// apart from this suite's by the identity the request presented.
const AUDIT_ACCESS_KEY_ID: &str = "AKIDSECURITYPLANEAUDIT";

/// What the `resource` field reads as when the call resolved no single resource
/// (`crate::state::AUDIT_NO_RESOURCE`), which is what a listing of a whole type records.
const NO_RESOURCE: &str = "-";

/// Regions a row is signed for, so a record is proven to be about the resource the request
/// addressed whichever Region asked.
const REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "ap-southeast-2"];

/// A `PolicyVersion`-shaped value this emulator never minted: 20 characters of the
/// modelled `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$` at
/// `PolicyVersion`'s own `@length(min: 20, max: 36)`, so it reaches the handler and is
/// refused there rather than at the boundary.
const FORGED_VERSION: &str = "AAAAAAAAAAAAAAAAAAAA";

/// An `encryption` security policy document: the `AWSOwnedKey` form the service's own
/// documentation uses for a collection pattern.
const ENCRYPTION_POLICY: &str =
    r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}"#;

/// A `network` security policy document: the public-access form, which is a LIST of rules.
const NETWORK_POLICY: &str = r#"[{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AllowFromPublic":true}]"#;

/// The `resource` field a record naming one security policy carries: the `(type, name)`
/// pair every security-policy request addresses one by.
fn policy_resource(kind: &str, name: &str) -> String {
    format!("security-policy/{kind}/{name}")
}

/// The `resource` field a record naming one security configuration carries: its
/// `SecurityConfigId`, which is `<type>/<account-id>/<name>`.
fn config_resource(kind: &str, name: &str) -> String {
    format!("security-config/{}", config_id(kind, name))
}

/// The id a configuration of `kind` named `name` is addressed by.
fn config_id(kind: &str, name: &str) -> String {
    format!("{kind}/{ACCOUNT}/{name}")
}

/// The records THIS suite's calls wrote, taken from the shared capture.
fn records(captured: &CapturedRecords) -> Vec<String> {
    captured.drain_for(AUDIT_ACCESS_KEY_ID)
}

/// One row of the table: the case, the Region it is signed for, the operation, the request,
/// the status it must answer, the `resource` its record must name and the outcome that
/// record must carry.
type Row = (String, &'static str, &'static str, Value, u16, String, &'static str);

/// RULE: every security-policy and security-configuration call — the read as much as the
/// write, accepted or refused — leaves exactly ONE structured audit record naming the
/// service, the operation, the identity the request presented, the request target, the
/// resource it resolved and the outcome.
#[tokio::test]
async fn every_security_plane_call_emits_one_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a missing record below can never be mistaken for a capture that
    // receives nothing: the capture must receive an event emitted right here.
    tracing::info!(
        target: "emulator::audit",
        access_key_id = AUDIT_ACCESS_KEY_ID,
        "probe"
    );
    let probe = records(captured);
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();

    // Phase 1: the reads and the refusals a store holding none of these resources answers.
    // A record is owed by the call, not by what the account happens to hold.
    let empty_store: Vec<Row> = vec![
        (
            "a policy read before anything was stored".to_string(),
            REGIONS[0],
            "GetSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-one" }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "not-found",
        ),
        (
            "an empty policy listing".to_string(),
            REGIONS[0],
            "ListSecurityPolicies",
            json!({ "type": "encryption" }),
            200,
            NO_RESOURCE.to_string(),
            "policy-read",
        ),
        (
            "a policy listing carrying a nextToken this service never issued".to_string(),
            REGIONS[1],
            "ListSecurityPolicies",
            json!({ "type": "network", "nextToken": "not-a-token" }),
            CLIENT_ERROR,
            NO_RESOURCE.to_string(),
            "validation-refused",
        ),
        (
            "a policy delete addressing nothing stored".to_string(),
            REGIONS[2],
            "DeleteSecurityPolicy",
            json!({ "type": "network", "name": "net-gone" }),
            CLIENT_ERROR,
            policy_resource("network", "net-gone"),
            "not-found",
        ),
        (
            "a policy update addressing nothing stored".to_string(),
            REGIONS[2],
            "UpdateSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-one", "policyVersion": FORGED_VERSION }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "not-found",
        ),
        (
            "a configuration read before anything was stored".to_string(),
            REGIONS[0],
            "GetSecurityConfig",
            json!({ "id": config_id("saml", "cfg-one") }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-one"),
            "not-found",
        ),
        (
            "an empty configuration listing".to_string(),
            REGIONS[0],
            "ListSecurityConfigs",
            json!({ "type": "saml" }),
            200,
            NO_RESOURCE.to_string(),
            "policy-read",
        ),
        (
            "a configuration listing carrying a nextToken this service never issued"
                .to_string(),
            REGIONS[1],
            "ListSecurityConfigs",
            json!({ "type": "saml", "nextToken": "not-a-token" }),
            CLIENT_ERROR,
            NO_RESOURCE.to_string(),
            "validation-refused",
        ),
        (
            "a configuration delete addressing nothing stored".to_string(),
            REGIONS[2],
            "DeleteSecurityConfig",
            json!({ "id": config_id("saml", "cfg-gone") }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-gone"),
            "not-found",
        ),
        (
            "a configuration update addressing nothing stored".to_string(),
            REGIONS[2],
            "UpdateSecurityConfig",
            json!({
                "id": config_id("saml", "cfg-gone"),
                "configVersion": FORGED_VERSION,
            }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-gone"),
            "not-found",
        ),
    ];

    // Phase 2: the writes that store these resources, and every refusal a create answers.
    let creates: Vec<Row> = vec![
        (
            "an encryption policy was created".to_string(),
            REGIONS[0],
            "CreateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policy": ENCRYPTION_POLICY,
                "clientToken": "token-enc-one",
            }),
            200,
            policy_resource("encryption", "enc-one"),
            "policy-created",
        ),
        (
            "the create was retried with the token it already spent".to_string(),
            REGIONS[0],
            "CreateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policy": ENCRYPTION_POLICY,
                "clientToken": "token-enc-one",
            }),
            200,
            policy_resource("encryption", "enc-one"),
            "idempotent-replay",
        ),
        (
            "a second create of the same pair".to_string(),
            REGIONS[1],
            "CreateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policy": ENCRYPTION_POLICY,
            }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "already-exists",
        ),
        (
            "a create whose policy text is not JSON".to_string(),
            REGIONS[1],
            "CreateSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-two", "policy": "not-json" }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-two"),
            "validation-refused",
        ),
        (
            "a network policy was created".to_string(),
            REGIONS[2],
            "CreateSecurityPolicy",
            json!({ "type": "network", "name": "net-one", "policy": NETWORK_POLICY }),
            200,
            policy_resource("network", "net-one"),
            "policy-created",
        ),
        (
            "a SAML configuration was created".to_string(),
            REGIONS[0],
            "CreateSecurityConfig",
            json!({
                "type": "saml",
                "name": "cfg-one",
                "samlOptions": saml_options(),
                "clientToken": "token-cfg-one",
            }),
            200,
            config_resource("saml", "cfg-one"),
            "policy-created",
        ),
        (
            "the configuration create was retried with the token it already spent".to_string(),
            REGIONS[0],
            "CreateSecurityConfig",
            json!({
                "type": "saml",
                "name": "cfg-one",
                "samlOptions": saml_options(),
                "clientToken": "token-cfg-one",
            }),
            200,
            config_resource("saml", "cfg-one"),
            "idempotent-replay",
        ),
        (
            "a second create of the same configuration id".to_string(),
            REGIONS[1],
            "CreateSecurityConfig",
            json!({ "type": "saml", "name": "cfg-one", "samlOptions": saml_options() }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-one"),
            "already-exists",
        ),
        (
            "a create naming no options for the type it asks for".to_string(),
            REGIONS[1],
            "CreateSecurityConfig",
            json!({ "type": "saml", "name": "cfg-two" }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-two"),
            "validation-refused",
        ),
        (
            "an IAM Identity Center configuration was created".to_string(),
            REGIONS[2],
            "CreateSecurityConfig",
            json!({
                "type": "iamidentitycenter",
                "name": "cfg-idc",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
            200,
            config_resource("iamidentitycenter", "cfg-idc"),
            "policy-created",
        ),
    ];

    for rows in [empty_store, creates] {
        for (case, region, operation, request, status, resource, outcome) in rows {
            audited(
                &router, captured, &case, region, operation, request, status, &resource, outcome,
            )
            .await;
        }
    }

    // Phase 3: the reads of what phase 2 stored, which are owed a record of their own, and
    // the versions the updates below need as their precondition.
    let policy = audited(
        &router,
        captured,
        "the stored encryption policy was read",
        REGIONS[0],
        "GetSecurityPolicy",
        json!({ "type": "encryption", "name": "enc-one" }),
        200,
        &policy_resource("encryption", "enc-one"),
        "policy-read",
    )
    .await;
    let policy_version = text(&policy, "securityPolicyDetail.policyVersion");

    let config = audited(
        &router,
        captured,
        "the stored SAML configuration was read",
        REGIONS[0],
        "GetSecurityConfig",
        json!({ "id": config_id("saml", "cfg-one") }),
        200,
        &config_resource("saml", "cfg-one"),
        "policy-read",
    )
    .await;
    let config_version = text(&config, "securityConfigDetail.configVersion");

    let writes_and_reads: Vec<Row> = vec![
        (
            "a policy update whose policyVersion is not the stored one".to_string(),
            REGIONS[1],
            "UpdateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policyVersion": FORGED_VERSION,
                "policy": ENCRYPTION_POLICY,
            }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "version-mismatch",
        ),
        (
            "a policy update whose policy text is not JSON".to_string(),
            REGIONS[1],
            "UpdateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policyVersion": policy_version,
                "policy": "{",
            }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "validation-refused",
        ),
        (
            "the stored policy was updated".to_string(),
            REGIONS[2],
            "UpdateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": "enc-one",
                "policyVersion": policy_version,
                "description": "the audited policy",
            }),
            200,
            policy_resource("encryption", "enc-one"),
            "policy-updated",
        ),
        (
            "a configuration update whose configVersion is not the stored one".to_string(),
            REGIONS[1],
            "UpdateSecurityConfig",
            json!({
                "id": config_id("saml", "cfg-one"),
                "configVersion": FORGED_VERSION,
                "samlOptions": saml_options(),
            }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-one"),
            "version-mismatch",
        ),
        (
            "a configuration update naming another type's options".to_string(),
            REGIONS[1],
            "UpdateSecurityConfig",
            json!({
                "id": config_id("saml", "cfg-one"),
                "configVersion": config_version,
                "iamIdentityCenterOptionsUpdates": {},
            }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-one"),
            "validation-refused",
        ),
        (
            "the stored configuration was updated".to_string(),
            REGIONS[2],
            "UpdateSecurityConfig",
            json!({
                "id": config_id("saml", "cfg-one"),
                "configVersion": config_version,
                "description": "the audited configuration",
            }),
            200,
            config_resource("saml", "cfg-one"),
            "policy-updated",
        ),
        (
            "the policies of a type were listed after the writes".to_string(),
            REGIONS[0],
            "ListSecurityPolicies",
            json!({ "type": "encryption", "maxResults": 1 }),
            200,
            NO_RESOURCE.to_string(),
            "policy-read",
        ),
        (
            "the configurations of a type were listed after the writes".to_string(),
            REGIONS[0],
            "ListSecurityConfigs",
            json!({ "type": "saml", "maxResults": 1 }),
            200,
            NO_RESOURCE.to_string(),
            "policy-read",
        ),
        (
            "the stored policy was deleted".to_string(),
            REGIONS[1],
            "DeleteSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-one" }),
            200,
            policy_resource("encryption", "enc-one"),
            "policy-deleted",
        ),
        (
            "the deleted policy was deleted again".to_string(),
            REGIONS[1],
            "DeleteSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-one" }),
            CLIENT_ERROR,
            policy_resource("encryption", "enc-one"),
            "not-found",
        ),
        (
            "the stored configuration was deleted".to_string(),
            REGIONS[2],
            "DeleteSecurityConfig",
            json!({ "id": config_id("saml", "cfg-one") }),
            200,
            config_resource("saml", "cfg-one"),
            "policy-deleted",
        ),
        (
            "the deleted configuration was deleted again".to_string(),
            REGIONS[2],
            "DeleteSecurityConfig",
            json!({ "id": config_id("saml", "cfg-one") }),
            CLIENT_ERROR,
            config_resource("saml", "cfg-one"),
            "not-found",
        ),
    ];
    for (case, region, operation, request, status, resource, outcome) in writes_and_reads {
        audited(
            &router, captured, &case, region, operation, request, status, &resource, outcome,
        )
        .await;
    }

    // Phase 4: the quota exit of each half. Every create on the way to the quota is owed its
    // own record too, so the filling loop is driven through the same assertion.
    // `net-one` above already holds one of the type's slots.
    for index in 1..MAX_SECURITY_POLICIES_PER_TYPE {
        let name = format!("net-{index:02}");
        audited(
            &router,
            captured,
            &format!("network policy {name} filling the type's quota"),
            REGIONS[0],
            "CreateSecurityPolicy",
            json!({ "type": "network", "name": name, "policy": NETWORK_POLICY }),
            200,
            &policy_resource("network", &name),
            "policy-created",
        )
        .await;
    }
    audited(
        &router,
        captured,
        "a network policy past the type's quota",
        REGIONS[0],
        "CreateSecurityPolicy",
        json!({ "type": "network", "name": "net-past", "policy": NETWORK_POLICY }),
        CLIENT_ERROR,
        &policy_resource("network", "net-past"),
        "quota-exceeded",
    )
    .await;

    // The SAML configuration created above was deleted, so the type holds none.
    for index in 0..MAX_SECURITY_CONFIGS_PER_TYPE {
        let name = format!("cfg-{index:02}");
        audited(
            &router,
            captured,
            &format!("SAML configuration {name} filling the type's quota"),
            REGIONS[0],
            "CreateSecurityConfig",
            json!({ "type": "saml", "name": name, "samlOptions": saml_options() }),
            200,
            &config_resource("saml", &name),
            "policy-created",
        )
        .await;
    }
    audited(
        &router,
        captured,
        "a SAML configuration past the type's quota",
        REGIONS[0],
        "CreateSecurityConfig",
        json!({ "type": "saml", "name": "cfg-past", "samlOptions": saml_options() }),
        CLIENT_ERROR,
        &config_resource("saml", "cfg-past"),
        "quota-exceeded",
    )
    .await;
}

/// Drive `operation` with `request` in `region`, assert it answered `expected_status`, and
/// assert the call left exactly ONE structured audit record naming `resource` and carrying
/// `outcome`, with none of the request's signature. Answers the response body.
#[allow(clippy::too_many_arguments)]
async fn audited(
    router: &smithy_server_runtime::MultiServiceRouter,
    captured: &CapturedRecords,
    case: &str,
    region: &str,
    operation: &str,
    request: Value,
    expected_status: u16,
    resource: &str,
    outcome: &str,
) -> Value {
    let (status, code, answer) =
        call_signed_as(router, region, AUDIT_ACCESS_KEY_ID, operation, request).await;
    assert_eq!(
        status, expected_status,
        "case {case}: expected {expected_status}, got {status} {code} with {answer}"
    );

    let written = records(captured);
    assert_eq!(
        written.len(),
        1,
        "case {case}: exactly one audit record per call, got {written:?}"
    );
    let record = &written[0];
    for expected in [
        format!("service={SERVICE}"),
        format!("operation={operation}"),
        // The resource and the outcome are asserted as the one adjacent pair the record
        // renders them as, so a resource that is merely a PREFIX of the expected one cannot
        // pass.
        format!("resource={resource} outcome={outcome}"),
        "request_target=POST /".to_string(),
        // The identity the request presented: this suite signs with this access key id, so
        // the field's CONTENT is checked, not merely its presence.
        format!("access_key_id={AUDIT_ACCESS_KEY_ID}"),
    ] {
        assert!(
            record.contains(&expected),
            "case {case}: the audit record must carry {expected:?}, got {record:?}"
        );
    }
    // Metadata only: neither the request's signature nor the policy document reaches the
    // log.
    for secret in ["deadbeef", "ResourceType", "metadata"] {
        assert!(
            !record.contains(secret),
            "case {case}: the audit record must not carry {secret:?}, got {record:?}"
        );
    }
    answer
}
