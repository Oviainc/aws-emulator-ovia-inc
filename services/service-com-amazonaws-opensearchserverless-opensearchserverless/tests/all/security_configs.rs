// SPDX-License-Identifier: CC0-1.0

//! OpenSearch Serverless security configurations end to end over the wire: the create
//! that stores one, the read that answers it by id, and the listing that reports the
//! stored ones of one type.
//!
//! Every case drives the real request path through
//! [`crate::collection_lifecycle::call`] — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the
//! boundary validator, the protocol codec and the handler all take part, exactly as they
//! do for an SDK client.
//!
//! The rule under test: `ListSecurityConfigs` reports the stored security configurations
//! of the `type` the request names, in one stable order, cutting at the page bound and
//! handing back a `nextToken` only while records remain — and a summary's members are the
//! stored record's, not the request's. The tables span what that rule generalises over:
//! every `SecurityConfigType`, the page bound from 1 to past the record count and up to
//! the published maximum, the record count, and each identity-provider option set.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text};

/// A minimal IdP metadata document: `samlMetadata` is
/// `@length(1..51200)` with `@pattern("[\u{0009}\u{000A}\u{000D}\u{0020}-\u{007E}\u{00A1}-\u{00FF}]+")`,
/// which is printable text.
const SAML_METADATA: &str =
    r#"<EntityDescriptor entityID="https://idp.example.com/metadata"></EntityDescriptor>"#;

/// An IAM Identity Center instance ARN of the modelled shape
/// (`IamIdentityCenterInstanceArn`'s
/// `^arn:(aws|aws-us-gov|aws-cn|aws-iso|aws-iso-b):sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`).
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// The `samlOptions` of a request whose `name` is `name`, carrying nothing optional.
fn saml_options() -> Value {
    json!({ "metadata": SAML_METADATA })
}

/// The id a configuration of `kind` named `name` carries: `<type>/<account-id>/<name>`.
fn expected_id(kind: &str, name: &str) -> String {
    format!("{kind}/{}/{name}", engine::emulator_account_id())
}

/// Create the security configuration `body` describes, asserting it was accepted, and
/// answer its `securityConfigDetail`.
async fn create_config(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, "CreateSecurityConfig", body).await;
    assert_eq!(
        status, 200,
        "case CreateSecurityConfig({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "securityConfigDetail").clone()
}

/// The summaries `ListSecurityConfigs` reports for `kind`, over every page.
async fn list_all(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "type": kind, "maxResults": 100 });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, code, answer) = call(router, "ListSecurityConfigs", request).await;
        assert_eq!(
            status, 200,
            "case ListSecurityConfigs({kind}): expected 200, got {status} {code} with {answer}"
        );
        summaries.extend(
            at(&answer, "securityConfigSummaries")
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        match at(&answer, "nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return summaries,
        }
    }
}

// ---------------------------------------------------------------------------
// The create/read round trip, over a table that spans every `SecurityConfigType`,
// each type's options, the documented defaults of the members a request may omit,
// and a name reused under another type.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_security_config_is_what_the_listing_of_its_type_reports() {
    let router = emulator();

    // `SecurityConfigType` = saml | iamidentitycenter | iamfederation (smithy_query shape
    // values). Row 1 names every optional SAML member; row 2 omits them, so it pins
    // `sessionTimeout`'s documented 60-minute default; rows 3 and 4 pin the IAM Identity
    // Center attribute defaults (`UserId`/`GroupId`) and then override both; row 5 is the
    // third type; row 6 reuses row 1's NAME under another type, which is a different
    // configuration because the id carries the type.
    let cases: Vec<(&str, &str, Value, Option<&str>, Value)> = vec![
        (
            "saml",
            "full-saml",
            json!({
                "type": "saml",
                "name": "full-saml",
                "description": "the saml provider",
                "samlOptions": {
                    "metadata": SAML_METADATA,
                    "userAttribute": "Email",
                    "groupAttribute": "Group",
                    "openSearchServerlessEntityId": format!("aws:opensearch:{}:", engine::emulator_account_id()),
                    "sessionTimeout": 720,
                },
            }),
            Some("the saml provider"),
            json!({
                "samlOptions": {
                    "metadata": SAML_METADATA,
                    "userAttribute": "Email",
                    "groupAttribute": "Group",
                    "openSearchServerlessEntityId": format!("aws:opensearch:{}:", engine::emulator_account_id()),
                    "sessionTimeout": 720,
                }
            }),
        ),
        (
            "saml",
            "default-saml",
            json!({ "type": "saml", "name": "default-saml", "samlOptions": saml_options() }),
            None,
            // `SamlConfigOptions$sessionTimeout`: "The session timeout, in minutes.
            // Default is 60 minutes."
            json!({ "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 60 } }),
        ),
        (
            "saml",
            "min-timeout-saml",
            json!({
                "type": "saml",
                "name": "min-timeout-saml",
                // `sessionTimeout`'s published minimum (`@range(min: 5, max: 720)`).
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 5 },
            }),
            None,
            json!({ "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 5 } }),
        ),
        (
            "iamidentitycenter",
            "default-idc",
            json!({
                "type": "iamidentitycenter",
                "name": "default-idc",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
            None,
            // `CreateIamIdentityCenterConfigOptions$userAttribute` "Defaults to UserId";
            // `$groupAttribute` "Defaults to GroupId".
            json!({
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    "userAttribute": "UserId",
                    "groupAttribute": "GroupId",
                }
            }),
        ),
        (
            "iamidentitycenter",
            "explicit-idc",
            json!({
                "type": "iamidentitycenter",
                "name": "explicit-idc",
                "description": "identity center with named attributes",
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    // `IamIdentityCenterUserAttribute` = Email | UserId | UserName and
                    // `IamIdentityCenterGroupAttribute` = GroupId | GroupName.
                    "userAttribute": "UserName",
                    "groupAttribute": "GroupName",
                },
            }),
            Some("identity center with named attributes"),
            json!({
                "iamIdentityCenterOptions": {
                    "instanceArn": INSTANCE_ARN,
                    "userAttribute": "UserName",
                    "groupAttribute": "GroupName",
                }
            }),
        ),
        (
            "iamfederation",
            "federated-config",
            json!({
                "type": "iamfederation",
                "name": "federated-config",
                "iamFederationOptions": { "userAttribute": "Email", "groupAttribute": "Department" },
            }),
            None,
            json!({
                "iamFederationOptions": { "userAttribute": "Email", "groupAttribute": "Department" }
            }),
        ),
        (
            // The same NAME as row 1 under another type: the (type, name) pair is the
            // identity, so this is a second configuration and not a conflict.
            "iamfederation",
            "full-saml",
            json!({
                "type": "iamfederation",
                "name": "full-saml",
                "iamFederationOptions": { "userAttribute": "Email" },
            }),
            None,
            json!({ "iamFederationOptions": { "userAttribute": "Email" } }),
        ),
    ];

    let mut versions: BTreeSet<String> = BTreeSet::new();
    let mut created_per_type: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();

    for (kind, name, request, expected_description, expected_options) in &cases {
        let case = format!("CreateSecurityConfig({kind}/{name})");
        let detail = create_config(&router, request.clone()).await;

        assert_eq!(
            text(&detail, "id"),
            expected_id(kind, name),
            "case {case}: the id is <type>/<account-id>/<name>, got {detail}"
        );
        assert_eq!(
            text(&detail, "type"),
            *kind,
            "case {case}: type must be the requested one, got {detail}"
        );
        let version = text(&detail, "configVersion");
        assert!(
            is_policy_version(&version),
            "case {case}: configVersion must match PolicyVersion's length and pattern, got {version:?}"
        );
        assert!(
            versions.insert(version.clone()),
            "case {case}: every write mints a fresh configVersion, got a repeat of {version:?}"
        );
        assert_eq!(
            text(&detail, "description"),
            expected_description.unwrap_or_default(),
            "case {case}: the stored description is reported, got {detail}"
        );
        for member in ["samlOptions", "iamIdentityCenterOptions", "iamFederationOptions"] {
            assert_eq!(
                at(&detail, member),
                at(expected_options, member),
                "case {case}: {member} must be the stored options, got {detail}"
            );
        }
        let created = at(&detail, "createdDate").as_i64().unwrap_or_default();
        assert!(
            created > 1_600_000_000_000,
            "case {case}: createdDate must be epoch milliseconds, got {created}"
        );
        assert_eq!(
            at(&detail, "lastModifiedDate").as_i64(),
            Some(created),
            "case {case}: a just-created configuration was last modified when it was created, got {detail}"
        );

        created_per_type
            .entry((*kind).to_string())
            .or_default()
            .push(detail);
    }

    // The read-back through `GetSecurityConfig` reports exactly what the create stored,
    // down to the configVersion and the timestamps.
    for details in created_per_type.values() {
        for detail in details {
            let id = text(detail, "id");
            let (status, code, read) =
                call(&router, "GetSecurityConfig", json!({ "id": id.clone() })).await;
            assert_eq!(
                status, 200,
                "case GetSecurityConfig({id}): expected 200, got {status} {code} with {read}"
            );
            assert_eq!(
                at(&read, "securityConfigDetail"),
                detail,
                "case GetSecurityConfig({id}): the read must answer the created record, got {read}"
            );
        }
    }

    // An id that names no stored configuration is the modelled 404, including the id a
    // stored NAME carries under a type it was never created under.
    for (case, id) in [
        ("an id of no configuration", expected_id("saml", "absent-config")),
        (
            "a stored name under another type",
            expected_id("iamidentitycenter", "full-saml"),
        ),
    ] {
        let (status, code, answer) = call(&router, "GetSecurityConfig", json!({ "id": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case GetSecurityConfig({case}): expected 404 ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }

    // The read-back: `ListSecurityConfigs` reports exactly the stored configurations of
    // the type it was asked for, and a summary's members are the detail's own.
    for kind in ["saml", "iamidentitycenter", "iamfederation"] {
        let expected = created_per_type.get(kind).cloned().unwrap_or_default();
        let listed = list_all(&router, kind).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "case ListSecurityConfigs({kind}): expected {} summaries, got {listed:?}",
            expected.len()
        );
        for detail in &expected {
            let id = text(detail, "id");
            let summary = listed
                .iter()
                .find(|summary| text(summary, "id") == id)
                .unwrap_or_else(|| {
                    panic!("case ListSecurityConfigs({kind}): {id} must be listed, got {listed:?}")
                });
            // `SecurityConfigSummary` is `SecurityConfigDetail` without the
            // identity-provider options, so every member it declares must be the one the
            // stored record reports.
            for member in [
                "id",
                "type",
                "configVersion",
                "description",
                "createdDate",
                "lastModifiedDate",
            ] {
                assert_eq!(
                    at(summary, member),
                    at(detail, member),
                    "case ListSecurityConfigs({kind}): summary.{member} must be the stored record's, got {summary}"
                );
            }
            for member in ["samlOptions", "iamIdentityCenterOptions", "iamFederationOptions"] {
                assert_eq!(
                    at(summary, member),
                    &Value::Null,
                    "case ListSecurityConfigs({kind}): a summary carries no {member}, got {summary}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ListSecurityConfigs: the page-bound invariant over a range of bounds and record
// counts, the type it is asked for, and the tokens it refuses.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_security_configs_pages_over_the_stored_records_of_one_type() {
    let router = emulator();

    // A type with nothing stored lists nothing — not an error and not another type's
    // records.
    for kind in ["saml", "iamidentitycenter", "iamfederation"] {
        let (status, code, answer) = call(&router, "ListSecurityConfigs", json!({ "type": kind })).await;
        assert_eq!(
            status, 200,
            "case ListSecurityConfigs({kind}, empty store): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "securityConfigSummaries"),
            &json!([]),
            "case ListSecurityConfigs({kind}, empty store): expected no summaries, got {answer}"
        );
        assert_eq!(
            at(&answer, "nextToken"),
            &Value::Null,
            "case ListSecurityConfigs({kind}, empty store): expected no nextToken, got {answer}"
        );
    }

    // Seven SAML configurations and three federation ones, so the page walk has to cut
    // inside one type while ignoring the other.
    let saml_total = 7;
    let mut saml_ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..saml_total {
        let name = format!("paged-saml-{index}");
        let detail = create_config(
            &router,
            json!({ "type": "saml", "name": name, "samlOptions": saml_options() }),
        )
        .await;
        saml_ids.insert(text(&detail, "id"));
    }
    let mut federation_ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..3 {
        let name = format!("paged-federation-{index}");
        let detail = create_config(
            &router,
            json!({ "type": "iamfederation", "name": name, "iamFederationOptions": {} }),
        )
        .await;
        federation_ids.insert(text(&detail, "id"));
    }

    // The pagination invariant, over every page bound from 1 to one past the record count
    // of the type plus the whole published maximum (`maxResults`' `@range(1..100)`):
    // walking the pages visits each stored record of that type exactly once, in one
    // order, and a `nextToken` comes back only while records remain.
    for (kind, expected) in [("saml", &saml_ids), ("iamfederation", &federation_ids)] {
        for bound in [1, 2, 3, 6, 7, 8, 100] {
            let case = format!("ListSecurityConfigs({kind}, maxResults={bound})");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                pages += 1;
                assert!(
                    pages <= expected.len() + 2,
                    "case {case}: pagination did not terminate after {pages} pages"
                );
                let mut request = json!({ "type": kind, "maxResults": bound });
                if let Some(token) = &token {
                    request["nextToken"] = json!(token);
                }
                let (status, code, answer) = call(&router, "ListSecurityConfigs", request).await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} {code} with {answer}"
                );
                let page = at(&answer, "securityConfigSummaries")
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= usize::try_from(bound).unwrap_or(1),
                    "case {case}: a page may hold at most {bound} records, got {}",
                    page.len()
                );
                for summary in &page {
                    assert_eq!(
                        text(summary, "type"),
                        kind,
                        "case {case}: a listing reports only its own type, got {summary}"
                    );
                    seen.push(text(summary, "id"));
                }
                match at(&answer, "nextToken").as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique.len(),
                seen.len(),
                "case {case}: no record may appear on two pages, got {seen:?}"
            );
            assert_eq!(
                &unique, expected,
                "case {case}: the pages together must be exactly the stored records of the type, got {seen:?}"
            );
            let mut sorted = seen.clone();
            sorted.sort();
            assert_eq!(
                seen, sorted,
                "case {case}: the pages must walk one stable order, got {seen:?}"
            );
        }
    }

    // With no `maxResults` the whole seven fit inside the documented default of 20, so the
    // listing is one page with no token.
    let (status, code, answer) = call(&router, "ListSecurityConfigs", json!({ "type": "saml" })).await;
    assert_eq!(
        status, 200,
        "case ListSecurityConfigs(no maxResults): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        at(&answer, "securityConfigSummaries")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        saml_total,
        "case ListSecurityConfigs(no maxResults): the default bound of 20 holds all {saml_total} records, got {answer}"
    );
    assert_eq!(
        at(&answer, "nextToken"),
        &Value::Null,
        "case ListSecurityConfigs(no maxResults): the last page carries no nextToken, got {answer}"
    );

    // What the listing refuses: a `nextToken` this service never issued, a `type` outside
    // the model's value list or in the wrong case, a missing `type`, and a `maxResults`
    // one past each published bound.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "forged nextToken",
            json!({ "type": "saml", "nextToken": "not-a-token" }),
        ),
        ("type outside SecurityConfigType", json!({ "type": "oidc" })),
        ("type in the wrong case", json!({ "type": "SAML" })),
        ("type absent", json!({})),
        (
            "maxResults one short of the minimum",
            json!({ "type": "saml", "maxResults": 0 }),
        ),
        (
            "maxResults one past the maximum",
            json!({ "type": "saml", "maxResults": 101 }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListSecurityConfigs", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case ListSecurityConfigs({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteSecurityConfig: the removal every later read sees, and the refusal an id
// that names nothing owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_security_config_removes_it_from_every_read() {
    let router = emulator();

    // One configuration of each type, so the removal is exercised over the whole value
    // list of `SecurityConfigType` and not just the recorded one.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "saml",
            "doomed-saml",
            json!({ "type": "saml", "name": "doomed-saml", "samlOptions": saml_options() }),
        ),
        (
            "iamidentitycenter",
            "doomed-idc",
            json!({
                "type": "iamidentitycenter",
                "name": "doomed-idc",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
        ),
        (
            "iamfederation",
            "doomed-federation",
            json!({
                "type": "iamfederation",
                "name": "doomed-federation",
                "iamFederationOptions": { "groupAttribute": "Department" },
            }),
        ),
    ];

    for (kind, name, request) in &cases {
        let case = format!("DeleteSecurityConfig({kind}/{name})");
        let detail = create_config(&router, request.clone()).await;
        let id = text(&detail, "id");

        let (status, code, answer) =
            call(&router, "DeleteSecurityConfig", json!({ "id": id.clone() })).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            answer,
            json!({}),
            "case {case}: the model declares no output members, so the body is empty, got {answer}"
        );

        // The record is gone: the read is a 404, the listing of its type no longer carries
        // it, and a second delete is a 404 too.
        let (status, code, read) =
            call(&router, "GetSecurityConfig", json!({ "id": id.clone() })).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a deleted configuration is not readable, got {status} {code} with {read}"
        );
        assert!(
            !list_all(&router, kind)
                .await
                .iter()
                .any(|summary| text(summary, "id") == id),
            "case {case}: a deleted configuration is no longer listed"
        );
        let (status, code, answer) =
            call(&router, "DeleteSecurityConfig", json!({ "id": id.clone() })).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}, second time: expected 404 ResourceNotFoundException, got {status} {code} with {answer}"
        );

        // And the (type, name) pair is free again, which is what shows the delete removed
        // the record rather than hiding it.
        let again = create_config(&router, request.clone()).await;
        assert_eq!(
            text(&again, "id"),
            id,
            "case {case}: recreating the pair mints the same id, got {again}"
        );
        assert_ne!(
            text(&again, "configVersion"),
            text(&detail, "configVersion"),
            "case {case}: the recreated configuration is a fresh record with its own configVersion, got {again}"
        );
    }

    // A `clientToken` whose configuration has been deleted cannot resurrect it: the replay
    // index never outlives the record, so the token creates a new configuration instead of
    // answering the deleted one.
    let first = create_config(
        &router,
        json!({
            "type": "saml",
            "name": "token-config",
            "samlOptions": saml_options(),
            "clientToken": "reused-token",
        }),
    )
    .await;
    let (status, code, answer) = call(
        &router,
        "DeleteSecurityConfig",
        json!({ "id": text(&first, "id") }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteSecurityConfig(token-config): expected 200, got {status} {code} with {answer}"
    );
    let second = create_config(
        &router,
        json!({
            "type": "saml",
            "name": "other-token-config",
            "samlOptions": saml_options(),
            "clientToken": "reused-token",
        }),
    )
    .await;
    assert_eq!(
        text(&second, "id"),
        expected_id("saml", "other-token-config"),
        "case CreateSecurityConfig(token of a deleted record): the token must not answer the deleted configuration, got {second}"
    );

    // An id of the right shape that names no configuration is a 404, not an empty success.
    let (status, code, answer) = call(
        &router,
        "DeleteSecurityConfig",
        json!({ "id": expected_id("saml", "never-created") }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case DeleteSecurityConfig(unknown id): expected 404 ResourceNotFoundException, got {status} {code} with {answer}"
    );
}

#[tokio::test]
async fn create_security_config_refuses_what_it_must_and_stops_at_the_quota() {
    let router = emulator();

    // Each options member is "required if you specify <type> for the type parameter", so
    // the options of the requested type are required and another type's are refused.
    // Beside them: the `name` bounds and pattern, each enum's value list, and
    // `sessionTimeout` one past each published bound.
    let refusals: Vec<(&str, Value)> = vec![
        ("type outside SecurityConfigType", json!({ "type": "oidc", "name": "bad-config" })),
        ("type in the wrong case", json!({ "type": "Saml", "name": "bad-config" })),
        ("type absent", json!({ "name": "bad-config", "samlOptions": saml_options() })),
        ("name absent", json!({ "type": "saml", "samlOptions": saml_options() })),
        ("samlOptions absent for type saml", json!({ "type": "saml", "name": "bad-config" })),
        (
            "iamIdentityCenterOptions absent for type iamidentitycenter",
            json!({ "type": "iamidentitycenter", "name": "bad-config" }),
        ),
        (
            "iamFederationOptions absent for type iamfederation",
            json!({ "type": "iamfederation", "name": "bad-config" }),
        ),
        (
            "another type's options",
            json!({
                "type": "saml",
                "name": "bad-config",
                "samlOptions": saml_options(),
                "iamFederationOptions": { "userAttribute": "Email" },
            }),
        ),
        (
            "name one short of the 3-character minimum",
            json!({ "type": "saml", "name": "ab", "samlOptions": saml_options() }),
        ),
        (
            "name one past the 32-character maximum",
            json!({ "type": "saml", "name": "a".repeat(33), "samlOptions": saml_options() }),
        ),
        (
            "name starting with a digit",
            json!({ "type": "saml", "name": "1bad-config", "samlOptions": saml_options() }),
        ),
        (
            "name with an uppercase letter",
            json!({ "type": "saml", "name": "Bad-config", "samlOptions": saml_options() }),
        ),
        (
            "name with an underscore",
            json!({ "type": "saml", "name": "bad_config", "samlOptions": saml_options() }),
        ),
        (
            "samlOptions without metadata",
            json!({ "type": "saml", "name": "bad-config", "samlOptions": { "userAttribute": "Email" } }),
        ),
        (
            "sessionTimeout one short of the 5-minute minimum",
            json!({
                "type": "saml",
                "name": "bad-config",
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 4 },
            }),
        ),
        (
            "sessionTimeout one past the 720-minute maximum",
            json!({
                "type": "saml",
                "name": "bad-config",
                "samlOptions": { "metadata": SAML_METADATA, "sessionTimeout": 721 },
            }),
        ),
        (
            "iamIdentityCenterOptions without instanceArn",
            json!({
                "type": "iamidentitycenter",
                "name": "bad-config",
                "iamIdentityCenterOptions": { "userAttribute": "UserId" },
            }),
        ),
        (
            "instanceArn outside IamIdentityCenterInstanceArn's pattern",
            json!({
                "type": "iamidentitycenter",
                "name": "bad-config",
                "iamIdentityCenterOptions": { "instanceArn": "arn:aws:sso:::instance/not-an-instance" },
            }),
        ),
        (
            "userAttribute outside IamIdentityCenterUserAttribute",
            json!({
                "type": "iamidentitycenter",
                "name": "bad-config",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN, "userAttribute": "Login" },
            }),
        ),
        (
            "groupAttribute outside IamIdentityCenterGroupAttribute",
            json!({
                "type": "iamidentitycenter",
                "name": "bad-config",
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN, "groupAttribute": "Team" },
            }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "CreateSecurityConfig", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case CreateSecurityConfig({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // Nothing above was stored: a refusal is not a write.
    for kind in ["saml", "iamidentitycenter", "iamfederation"] {
        assert!(
            list_all(&router, kind).await.is_empty(),
            "case ListSecurityConfigs({kind}): a refused create stores nothing"
        );
    }

    // The three `name` values just inside the published bounds are accepted, so the
    // refusals above are the bounds and not a blanket rejection.
    for name in ["abc", &"a".repeat(32), "a-b-c-1-2-3"] {
        create_config(
            &router,
            json!({ "type": "saml", "name": name, "samlOptions": saml_options() }),
        )
        .await;
    }

    // A second create for a (type, name) already stored is the conflict, and it leaves the
    // stored configuration alone.
    let before = list_all(&router, "saml").await;
    let (status, code, answer) = call(
        &router,
        "CreateSecurityConfig",
        json!({ "type": "saml", "name": "abc", "samlOptions": saml_options(), "description": "another" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateSecurityConfig(duplicate pair): expected 409 ConflictException, got {status} {code} with {answer}"
    );
    assert_eq!(
        list_all(&router, "saml").await,
        before,
        "case CreateSecurityConfig(duplicate pair): the stored configurations are unchanged"
    );

    // A replayed `clientToken` answers the record the first call created rather than
    // creating or conflicting with a second one; a different token for the same name is a
    // genuinely new request, so it conflicts.
    let request = json!({
        "type": "saml",
        "name": "idempotent-config",
        "samlOptions": saml_options(),
        "clientToken": "config-token",
    });
    let first = create_config(&router, request.clone()).await;
    let second = create_config(&router, request).await;
    assert_eq!(
        first, second,
        "case CreateSecurityConfig(replayed clientToken): the retry must answer the first record, got {first} and {second}"
    );
    let (status, code, answer) = call(
        &router,
        "CreateSecurityConfig",
        json!({
            "type": "saml",
            "name": "idempotent-config",
            "samlOptions": saml_options(),
            "clientToken": "other-token",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateSecurityConfig(new token, taken name): expected 409 ConflictException, got {status} {code} with {answer}"
    );

    // The quota, which is counted per `SecurityConfigType`: creates are accepted up to it
    // and the one past it is a 402 `ServiceQuotaExceededException` whose message names the
    // requested type and the limit. The table spans two types, so the count and the
    // message are per type and not one account-wide total.
    let quota = service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_SECURITY_CONFIGS_PER_TYPE;
    for (kind, options_member, options) in [
        ("saml", "samlOptions", saml_options()),
        ("iamfederation", "iamFederationOptions", json!({ "userAttribute": "Email" })),
    ] {
        let stored = list_all(&router, kind).await.len();
        for index in stored..quota {
            let mut request = json!({ "type": kind, "name": format!("quota-{kind}-{index}") });
            request[options_member] = options.clone();
            create_config(&router, request).await;
        }
        assert_eq!(
            list_all(&router, kind).await.len(),
            quota,
            "case CreateSecurityConfig({kind}, up to the quota): every create below the quota is accepted"
        );

        let mut request = json!({ "type": kind, "name": format!("past-the-{kind}-quota") });
        request[options_member] = options.clone();
        let (status, code, answer) = call(&router, "CreateSecurityConfig", request).await;
        assert_eq!(
            (status, code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case CreateSecurityConfig({kind}, one past the quota): expected 402 ServiceQuotaExceededException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("Your request exceeds the limit for {kind} policies in this AWS account. The limit is {quota}."),
            "case CreateSecurityConfig({kind}, one past the quota): the message names the type and the limit, got {answer}"
        );
        assert_eq!(
            list_all(&router, kind).await.len(),
            quota,
            "case CreateSecurityConfig({kind}, one past the quota): the refused create stored nothing"
        );

        // The other types are untouched by this one's quota, so each carries its own
        // count: `iamidentitycenter` still accepts a create.
        let name = format!("after-{kind}-quota");
        create_config(
            &router,
            json!({
                "type": "iamidentitycenter",
                "name": name,
                "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN },
            }),
        )
        .await;
    }
    assert_eq!(
        list_all(&router, "iamidentitycenter").await.len(),
        2,
        "case CreateSecurityConfig(a third type at two quotas): each type carries its own count"
    );
}
