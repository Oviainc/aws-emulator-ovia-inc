// SPDX-License-Identifier: CC0-1.0

//! The REGION the ARN of a NESTED resource carries, driven over the wire
//! (restJson1) through the same router the emulator boots.
//!
//! RULE: an ARN names the Region its resource lives in, and a resource nested
//! under another lives wherever its parent does — so the Region in a child's ARN
//! is the Region in its parent's ARN, whichever Region's endpoint the create
//! call that minted it was signed for. Every ARN in one response body therefore
//! names one Region, and the child's ARN really is nested under its parent's.
//!
//! The harness (the router, `Answer`, `call_in_region`, `encode`, `arn_of`,
//! `uuid_of`) is the one `app_bundle_ingestion_lifecycle` already builds; only
//! the Region tables and the chain helpers are new here.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// The account segment of every ARN a generated emulator emits.
const ACCOUNT: &str = "000000000000";

/// The Region component of an ARN: `arn:<partition>:<service>:<region>:<account>:<resource>`,
/// so the fourth colon-separated field.
fn arn_region(arn: &str) -> &str {
    arn.split(':')
        .nth(3)
        .unwrap_or_else(|| panic!("expected an ARN with a Region field, got {arn:?}"))
}

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type for your request"). The two values are the model's
/// `AuthType` enum (`smithy_query(query="shape", shape="AuthType")`: `apiKey` |
/// `oauth2`).
fn credential_for(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
    } else {
        json!({"oauth2Credential": {
            "clientId": "placeholder-client-id",
            "clientSecret": "placeholder-client-secret",
        }})
    }
}

/// The body `ConnectAppAuthorization` takes for one `authType`: an `oauth2`
/// authorization is completed with the code its consent step handed back, an
/// `apiKey` one carries its credential already.
fn connect_body(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({})
    } else {
        json!({"authRequest": {
            "redirectUri": "https://example.com/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
    }
}

/// `CreateAppAuthorization` under `bundle`, signed for `region`.
async fn create_authorization(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential_for(auth_type),
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
}

/// `ConnectAppAuthorization`, signed for `region` — the handshake AppFabric's
/// chain puts before an ingestion of the same (app, tenant) pair.
async fn connect_authorization(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    authorization: &str,
    auth_type: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(connect_body(auth_type)),
        )
        .await
}

/// `CreateIngestion` under `bundle`, signed for `region`.
async fn create_ingestion(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    app: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
            "POST",
            &format!("/appbundles/{}/ingestions", encode(bundle)),
            Some(json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"})),
        )
        .await
}

/// `CreateIngestionDestination` under `ingestion`, signed for `region`. The
/// `schema` and `format` values are the model's own closed enums (`Schema =
/// ocsf | raw`, `Format = json | parquet`).
async fn create_destination(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    ingestion: &str,
    bucket: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
            "POST",
            &format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(bundle),
                encode(ingestion)
            ),
            Some(json!({
                "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                "destinationConfiguration": {"auditLog": {"destination": {
                    "s3Bucket": {"bucketName": bucket}
                }}},
            })),
        )
        .await
}

/// A `GET` of one resource, signed for `region`.
async fn read(emulator: &Emulator, region: &str, uri: &str) -> Answer {
    emulator.call_in_region(region, "GET", uri, None).await
}

/// One member of a response body's named resource.
fn member(answer: &Answer, resource: &str, name: &str) -> String {
    answer
        .body
        .get(resource)
        .and_then(|value| value.get(name))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected {resource}.{name} in {}", answer.body))
        .to_string()
}

/// Assert a create answered HTTP 201.
fn created(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {}",
        answer.status, answer.body
    );
}

/// Assert `child` is the ARN of a resource nested under `parent`: same Region,
/// and `parent`'s resource path is the prefix of `child`'s.
fn assert_nested_under(case: &str, child: &str, parent: &str) {
    assert_eq!(
        arn_region(child),
        arn_region(parent),
        "case {case:?}: a nested resource lives where its parent does, so {child:?} must carry \
         the Region of {parent:?}"
    );
    assert!(
        child.starts_with(&format!("{parent}/")),
        "case {case:?}: expected {child:?} to be nested under {parent:?}"
    );
}

/// RULE: every child a create call mints under a stored parent carries that
/// PARENT's Region in its ARN — the create call's own signing Region never
/// appears in it — so the resource chain in one response body names one Region
/// and the child ARN really is nested under the parent ARN beside it.
///
/// The table spans the Region the parent lives in, the Region the child's create
/// call is signed for (equal to the parent's and different from it, in both
/// directions) and the applications the `app` member documents, and it drives all
/// three operations that mint a resource under an app bundle
/// (`CreateAppAuthorization`, `CreateIngestion`, `CreateIngestionDestination`).
/// Every row authorizes with an `apiKey`, because the ingestion each row mints
/// needs a pair whose connect handshake completed and an `oauth2` one never does
/// in this emulator (see `app_authorization_connect_validation`); the `authUrl` an
/// `oauth2` authorization reports for its own Region is covered by
/// `app_authorization_auth_url`.
#[tokio::test]
async fn a_nested_resource_carries_its_parents_region() {
    // (case, the Region the bundle lives in, the Region the child creates are
    // signed for, app, authType, tenant)
    let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
        (
            "children minted through the bundle's own endpoint",
            "us-east-1",
            "us-east-1",
            "SLACK",
            "apiKey",
            "t-1",
        ),
        (
            "a bundle in us-west-2, children minted through us-east-1",
            "us-west-2",
            "us-east-1",
            "ASANA",
            "apiKey",
            "tenant-with-a-much-longer-identifier",
        ),
        (
            "a bundle in us-east-1, children minted through eu-west-1",
            "us-east-1",
            "eu-west-1",
            "M365AUDITLOGS",
            "apiKey",
            "0",
        ),
        (
            "a bundle in ap-southeast-2, children minted through ca-central-1",
            "ap-southeast-2",
            "ca-central-1",
            "CISCO",
            "apiKey",
            "tenant.with.dots",
        ),
        (
            "a bundle in eu-central-1, children minted through ap-northeast-1",
            "eu-central-1",
            "ap-northeast-1",
            "SMARTSHEET",
            "apiKey",
            "tenant-s",
        ),
    ];

    for (case, bundle_region, call_region, app, auth_type, tenant) in cases {
        // A fresh store per row: a Region holds only one app bundle.
        let emulator = Emulator::new();
        let bundle_answer = emulator
            .create_app_bundle_in_region(bundle_region, json!({}))
            .await;
        created(&bundle_answer, case);
        let bundle = arn_of(&bundle_answer.body, "appBundle");
        assert_eq!(
            bundle,
            format!(
                "arn:aws:appfabric:{bundle_region}:{ACCOUNT}:appbundle/{}",
                uuid_of(&bundle)
            ),
            "case {case:?}: a bundle carries the Region it was created in, got {bundle:?}"
        );

        // 1. CreateAppAuthorization, signed for another endpoint than the
        //    bundle's.
        let authorization_answer =
            create_authorization(&emulator, call_region, &bundle, app, auth_type, tenant).await;
        created(&authorization_answer, case);
        let authorization = member(
            &authorization_answer,
            "appAuthorization",
            "appAuthorizationArn",
        );
        assert_eq!(
            member(&authorization_answer, "appAuthorization", "appBundleArn"),
            bundle,
            "case {case:?}: expected the stored parent ARN in {}",
            authorization_answer.body
        );
        assert_eq!(
            authorization,
            format!(
                "arn:aws:appfabric:{bundle_region}:{ACCOUNT}:appbundle/{}/appauthorization/{}",
                uuid_of(&bundle),
                uuid_of(&authorization)
            ),
            "case {case:?}: expected an authorization ARN nested in {bundle_region:?}, got \
             {authorization:?}"
        );
        assert_nested_under(case, &authorization, &bundle);
        // The URL an OAuth authorization reports is the endpoint of the Region
        // it lives in, which is the Region its ARN carries.
        if let Some(url) = authorization_answer
            .body
            .get("appAuthorization")
            .and_then(|value| value.get("authUrl"))
            .and_then(Value::as_str)
        {
            assert!(
                url.starts_with(&format!("https://appfabric.{bundle_region}.")),
                "case {case:?}: expected the consent URL at the {bundle_region:?} endpoint, got \
                 {url:?}"
            );
        }

        // authorize → connect → ingest.
        let connected = connect_authorization(
            &emulator,
            call_region,
            &bundle,
            &authorization,
            auth_type,
        )
        .await;
        assert_eq!(
            connected.status, 200,
            "case {case:?}: expected the connect handshake to succeed, got {} with body {}",
            connected.status, connected.body
        );

        // 2. CreateIngestion, signed for the same other endpoint.
        let ingestion_answer =
            create_ingestion(&emulator, call_region, &bundle, app, tenant).await;
        created(&ingestion_answer, case);
        let ingestion = arn_of(&ingestion_answer.body, "ingestion");
        assert_eq!(
            member(&ingestion_answer, "ingestion", "appBundleArn"),
            bundle,
            "case {case:?}: expected the stored parent ARN in {}",
            ingestion_answer.body
        );
        assert_eq!(
            ingestion,
            format!(
                "arn:aws:appfabric:{bundle_region}:{ACCOUNT}:appbundle/{}/ingestion/{}",
                uuid_of(&bundle),
                uuid_of(&ingestion)
            ),
            "case {case:?}: expected an ingestion ARN nested in {bundle_region:?}, got \
             {ingestion:?}"
        );
        assert_nested_under(case, &ingestion, &bundle);

        // 3. CreateIngestionDestination, one level deeper.
        let destination_answer = create_destination(
            &emulator,
            call_region,
            &bundle,
            &ingestion,
            "emulator-audit-logs",
        )
        .await;
        created(&destination_answer, case);
        let destination = arn_of(&destination_answer.body, "ingestionDestination");
        assert_eq!(
            member(&destination_answer, "ingestionDestination", "ingestionArn"),
            ingestion,
            "case {case:?}: expected the stored parent ARN in {}",
            destination_answer.body
        );
        assert_eq!(
            destination,
            format!(
                "arn:aws:appfabric:{bundle_region}:{ACCOUNT}:appbundle/{}/ingestion/{}/ingestiondestination/{}",
                uuid_of(&bundle),
                uuid_of(&ingestion),
                uuid_of(&destination)
            ),
            "case {case:?}: expected a destination ARN nested in {bundle_region:?}, got \
             {destination:?}"
        );
        assert_nested_under(case, &destination, &ingestion);

        // ONE Region across the whole chain: no response body names two.
        let regions: BTreeSet<&str> = [
            bundle.as_str(),
            authorization.as_str(),
            ingestion.as_str(),
            destination.as_str(),
        ]
        .into_iter()
        .map(arn_region)
        .collect();
        assert_eq!(
            regions,
            BTreeSet::from([*bundle_region]),
            "case {case:?}: the whole chain must name one Region, got {regions:?}"
        );

        // Round trip: each minted ARN names a resource the read path serves, and
        // the ARNs it reports are the ones the create call returned. (The
        // authorization's own status moved in between, through the connect
        // handshake above, so only its ARN members are compared; the two
        // records nothing touched are compared whole.)
        let reads = [
            (
                "appAuthorization",
                format!(
                    "/appbundles/{}/appauthorizations/{}",
                    encode(&bundle),
                    encode(&authorization)
                ),
                &authorization_answer,
                vec![
                    ("appAuthorizationArn", authorization.clone()),
                    ("appBundleArn", bundle.clone()),
                ],
                false,
            ),
            (
                "ingestion",
                format!(
                    "/appbundles/{}/ingestions/{}",
                    encode(&bundle),
                    encode(&ingestion)
                ),
                &ingestion_answer,
                vec![
                    ("arn", ingestion.clone()),
                    ("appBundleArn", bundle.clone()),
                ],
                true,
            ),
            (
                "ingestionDestination",
                format!(
                    "/appbundles/{}/ingestions/{}/ingestiondestinations/{}",
                    encode(&bundle),
                    encode(&ingestion),
                    encode(&destination)
                ),
                &destination_answer,
                vec![
                    ("arn", destination.clone()),
                    ("ingestionArn", ingestion.clone()),
                ],
                true,
            ),
        ];
        for (resource, uri, minted, arns, whole_record) in reads {
            let answer = read(&emulator, call_region, &uri).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200 reading {resource} at {uri:?}, got {} with \
                 body {}",
                answer.status, answer.body
            );
            for (name, expected) in arns {
                assert_eq!(
                    member(&answer, resource, name),
                    expected,
                    "case {case:?}: expected the stored {resource}.{name} {expected:?}, got {}",
                    answer.body
                );
            }
            if whole_record {
                assert_eq!(
                    answer.body.get(resource),
                    minted.body.get(resource),
                    "case {case:?}: expected the stored {resource} {}, got {}",
                    minted.body,
                    answer.body
                );
            }
        }
    }
}

/// RULE, as an INVARIANCE: the Region a child's ARN carries does not depend on
/// the endpoint the create call reached — several children of ONE parent, minted
/// through several different Regions' endpoints, all carry the parent's Region,
/// and every one of those ARNs names a resource this emulator serves (it
/// resolves through the tagging surface, which takes a bare ARN).
///
/// The table spans the Region the parent lives in and the number of endpoints
/// the children are minted through.
#[tokio::test]
async fn the_signing_region_never_reaches_a_nested_arn() {
    // The endpoints the children of one bundle are minted through — Regions
    // AppFabric is served in, none of them the first bundle Region below.
    const CALL_REGIONS: &[&str] = &["eu-west-2", "us-east-1", "ap-southeast-1", "eu-west-1"];

    for bundle_region in ["eu-west-2", "us-west-2", "ap-northeast-1"] {
        let emulator = Emulator::new();
        let bundle_answer = emulator
            .create_app_bundle_in_region(bundle_region, json!({}))
            .await;
        created(&bundle_answer, bundle_region);
        let bundle = arn_of(&bundle_answer.body, "appBundle");

        for (index, call_region) in CALL_REGIONS.iter().enumerate() {
            let case = format!("a {bundle_region} bundle, child minted through {call_region}");
            // One (app, tenant) pair per child: a bundle holds at most one
            // authorization per pair.
            let tenant = format!("tenant-{index}");
            let authorization_answer = create_authorization(
                &emulator,
                call_region,
                &bundle,
                "ZENDESK",
                "apiKey",
                &tenant,
            )
            .await;
            created(&authorization_answer, &case);
            let authorization = member(
                &authorization_answer,
                "appAuthorization",
                "appAuthorizationArn",
            );
            connect_authorization(
                &emulator,
                call_region,
                &bundle,
                &authorization,
                "apiKey",
            )
            .await;
            let ingestion_answer =
                create_ingestion(&emulator, call_region, &bundle, "ZENDESK", &tenant).await;
            created(&ingestion_answer, &case);
            let ingestion = arn_of(&ingestion_answer.body, "ingestion");

            for child in [authorization.as_str(), ingestion.as_str()] {
                assert_eq!(
                    arn_region(child),
                    bundle_region,
                    "case {case:?}: the signing Region must not reach a nested ARN, got {child:?}"
                );
                assert!(
                    !child.contains(&format!(":{call_region}:")) || *call_region == bundle_region,
                    "case {case:?}: {child:?} must not name the endpoint it was minted through"
                );

                // The minted ARN names a real resource: the tagging surface
                // resolves it by ARN alone.
                let tagged = emulator
                    .call_in_region(
                        call_region,
                        "POST",
                        &format!("/tags/{}", encode(child)),
                        Some(json!({"tags": [{"key": "chain", "value": "nested"}]})),
                    )
                    .await;
                assert_eq!(
                    tagged.status, 200,
                    "case {case:?}: expected {child:?} to resolve for TagResource, got {} with \
                     body {}",
                    tagged.status, tagged.body
                );
                let listed = read(&emulator, call_region, &format!("/tags/{}", encode(child))).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected HTTP 200 listing the tags of {child:?}, got {} with \
                     body {}",
                    listed.status, listed.body
                );
                assert_eq!(
                    listed.body.get("tags"),
                    Some(&json!([{"key": "chain", "value": "nested"}])),
                    "case {case:?}: expected the tag written through {child:?}, got {}",
                    listed.body
                );
            }
        }
    }
}
