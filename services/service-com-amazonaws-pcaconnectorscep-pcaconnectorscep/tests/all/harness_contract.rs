// SPDX-License-Identifier: CC0-1.0

//! The shared harness is driven end-to-end, through an operation from each suite that used
//! to carry its own copy of these rules.
//!
//! This suite is about [`crate::harness`] itself: the `@httpLabel` percent-encoding rule, the
//! signed-request path every suite reaches the service through, and the pending-transition
//! clock move with both of its intents (hold a transient status open, make it due) and both
//! of its contracts (require a transition to be owed, or tolerate there being none). Each
//! case drives the harness through a real operation — `CreateConnector`, `GetConnector`,
//! `ListConnectors`, `CreateChallenge`, `ListChallengeMetadata`, `TagResource`,
//! `ListTagsForResource`, `DeleteConnector` — so a drift in the harness fails here rather
//! than quietly changing what another suite's case does.

use serde_json::{json, Value};

use crate::harness::{self, label, new_router};

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-central-1";

/// A second Region, for the case that addresses one Region's endpoint while signing for
/// another.
const OTHER_REGION: &str = "ap-northeast-1";

/// The `ConnectorStatus` values the clock moves a connector between, named from the service's
/// own constants rather than respelled here.
const STATUS_CREATING: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::STATUS_CREATING;
const STATUS_ACTIVE: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::STATUS_ACTIVE;

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`), in the
/// Region the call naming it is signed for.
fn ca_arn(region: &str) -> String {
    format!(
        "arn:aws:acm-pca:{region}:000000000000:certificate-authority/\
         11111111-2222-3333-4444-555555555555"
    )
}

/// The percent-encoding rule, row by row.
///
/// The unreserved set an SDK leaves alone in a path segment is RFC 3986's: the ASCII letters
/// and digits plus `-`, `_`, `.` and `~`. The rows differ along what the rule generalises
/// over: which characters the value carries (unreserved, reserved, a space), the bytes a
/// non-ASCII character encodes to, the length, and the empty value.
#[tokio::test]
async fn the_label_rule_escapes_every_byte_outside_the_unreserved_set() {
    let cases: &[(&str, &str, &str)] = &[
        ("empty", "", ""),
        (
            "every unreserved character class",
            "aZ09-_.~",
            "aZ09-_.~",
        ),
        ("the ARN separators", "arn:aws", "arn%3Aaws"),
        ("a path separator", "connector/11", "connector%2F11"),
        ("a space", "a b", "a%20b"),
        // `+` and `=` are what a base64 `NextToken` carries, and both must stop being
        // query-string syntax.
        ("token padding", "tok+en==", "tok%2Ben%3D%3D"),
        // U+00E9 is two UTF-8 bytes, so the rule escapes each of them.
        ("a non-ASCII character", "caf\u{e9}", "caf%C3%A9"),
    ];

    for (case, value, expected) in cases {
        let encoded = label(value);
        assert_eq!(
            &encoded, expected,
            "case {case:?}: expected {value:?} to encode as {expected:?}, got {encoded:?}"
        );
    }

    // And the whole point of the rule: the encoded ARN is ONE path segment the service
    // resolves, while the same ARN sent raw is not that request at all.
    let (router, engine) = new_router();
    let created = harness::call(
        &router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(REGION) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"CreateConnector\": expected 202, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending(&engine, &arn);

    let encoded = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(&arn)),
        None,
    )
    .await;
    assert_eq!(
        encoded.status, 200,
        "case \"encoded label\": expected 200 from GetConnector, got {} / {:?}",
        encoded.status, encoded.body
    );
    let raw = harness::call(&router, REGION, "GET", &format!("/connectors/{arn}"), None).await;
    assert_ne!(
        raw.status, 200,
        "case \"unencoded label\": the `:` and `/` of an ARN must not read as path structure, \
         got {} / {:?}",
        raw.status, raw.body
    );
}

/// The shared request path and the shared clock move, driven through an operation from each
/// suite that used to hold a private copy of them.
#[tokio::test]
async fn the_shared_path_drives_an_operation_from_every_former_copy() {
    let (router, engine) = new_router();

    // CreateConnector: the ARN names the Region the call was signed for, which is what the
    // harness's credential scope carries.
    let created = harness::call(
        &router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(REGION), "Tags": { "suite": "harness" } })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"CreateConnector\": expected 202, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");
    assert!(
        connector.contains(&format!(":{REGION}:")),
        "case \"CreateConnector\": the ARN must name the signed-for Region {REGION}, got \
         {connector:?}"
    );

    // Holding the transition open: every reader reports the transient status, with no sleep
    // and no race against the service's own span.
    harness::hold_pending(&engine, &connector);
    let creating = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        creating.status, 200,
        "case \"GetConnector while CREATING\": expected 200, got {} / {:?}",
        creating.status, creating.body
    );
    assert_eq!(
        creating.connector("Status"),
        &json!(STATUS_CREATING),
        "case \"GetConnector while CREATING\": expected {STATUS_CREATING}, got {:?}",
        creating.body
    );

    // Making it due: the next call settles it, and nothing else on the record moved.
    harness::release_pending(&engine, &connector);
    let active = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        active.connector("Status"),
        &json!(STATUS_ACTIVE),
        "case \"GetConnector after the transition came due\": expected {STATUS_ACTIVE}, got {:?}",
        active.body
    );
    assert_eq!(
        active.connector("CertificateAuthorityArn"),
        &json!(ca_arn(REGION)),
        "case \"GetConnector after the transition came due\": the clock move must touch nothing \
         but the due instant, got {:?}",
        active.body
    );

    // ListConnectors: the same record, through the listing.
    let page = harness::call(&router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        page.count("Connectors"),
        1,
        "case \"ListConnectors\": expected the one created connector, got {:?}",
        page.body
    );

    // CreateChallenge: only a settled connector can mint one, so this also proves the clock
    // move landed.
    let challenge = harness::call(
        &router,
        REGION,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": connector })),
    )
    .await;
    assert_eq!(
        challenge.status, 202,
        "case \"CreateChallenge\": expected 202, got {} / {:?}",
        challenge.status, challenge.body
    );
    let challenge_arn = challenge
        .challenge("Arn")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"CreateChallenge\": expected a challenge Arn, got {:?}",
                challenge.body
            )
        })
        .to_string();
    assert!(
        !challenge
            .challenge("Password")
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "case \"CreateChallenge\": expected a password, got {:?}",
        challenge.body
    );

    // ListChallengeMetadata: a query-string value goes through the same encoding rule.
    let listed = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/challengeMetadata?ConnectorArn={}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        listed.count("Challenges"),
        1,
        "case \"ListChallengeMetadata\": expected the one minted challenge, got {:?}",
        listed.body
    );

    // TagResource and ListTagsForResource: a body-carrying POST and a read, both addressing
    // the challenge by its encoded ARN.
    let tagged = harness::call(
        &router,
        REGION,
        "POST",
        &format!("/tags/{}", label(&challenge_arn)),
        Some(json!({ "Tags": { "owner": "harness" } })),
    )
    .await;
    assert_eq!(
        tagged.status, 204,
        "case \"TagResource\": expected 204, got {} / {:?}",
        tagged.status, tagged.body
    );
    assert!(
        tagged.raw.is_empty(),
        "case \"TagResource\": the modelled answer carries no body, got {:?}",
        String::from_utf8_lossy(&tagged.raw)
    );
    let tags = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/tags/{}", label(&challenge_arn)),
        None,
    )
    .await;
    assert_eq!(
        tags.tags().get("owner").and_then(Value::as_str),
        Some("harness"),
        "case \"ListTagsForResource\": expected the tag TagResource stored, got {:?}",
        tags.body
    );

    // DeleteConnector: the teardown is a pending transition too, so the same clock move
    // drives it, and the refusal that follows carries the modelled error code and message.
    let deleted = harness::call(
        &router,
        REGION,
        "DELETE",
        &format!("/connectors/{}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        deleted.status, 202,
        "case \"DeleteConnector\": expected 202, got {} / {:?}",
        deleted.status, deleted.body
    );
    harness::release_pending(&engine, &connector);
    let gone = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        (gone.status, gone.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetConnector once the teardown came due\": expected a 404 \
         ResourceNotFoundException, got {} / {:?}",
        gone.status,
        gone.body
    );
    assert!(
        gone.message().contains(connector.as_str()),
        "case \"GetConnector once the teardown came due\": the refusal must name the ARN it was \
         asked for, got {:?}",
        gone.message()
    );
}

/// The tolerant contract: a case whose record may already have settled, or whose teardown may
/// already have completed, asks for the clock move and is told whether anything was owed.
#[tokio::test]
async fn the_clock_move_reports_when_nothing_is_owed_any_more() {
    let (router, engine) = new_router();

    let created = harness::call(
        &router,
        REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(REGION) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"CreateConnector\": expected 202, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");

    // Right after the create a transition IS owed, whichever intent asks for it.
    assert!(
        harness::hold_pending_if_owed(&engine, &connector),
        "case \"freshly created\": the clock owes the connector its settled status, got nothing \
         for {connector:?}"
    );
    assert!(
        harness::release_pending_if_owed(&engine, &connector),
        "case \"freshly created\": the clock move must still find the transition it just held, \
         got nothing for {connector:?}"
    );

    // Once the next call has settled it, nothing is owed: the tolerant helpers answer
    // `false` instead of failing the case.
    let settled = harness::call(
        &router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(&connector)),
        None,
    )
    .await;
    assert_eq!(
        settled.connector("Status"),
        &json!(STATUS_ACTIVE),
        "case \"settled\": expected {STATUS_ACTIVE}, got {:?}",
        settled.body
    );
    assert!(
        !harness::release_pending_if_owed(&engine, &connector),
        "case \"settled\": nothing is owed a settled connector, got a moved transition for \
         {connector:?}"
    );

    // And an ARN this service holds no record under is owed nothing either.
    let never_created = format!(
        "arn:aws:pca-connector-scep:{REGION}:000000000000:connector/\
         aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    );
    assert!(
        !harness::hold_pending_if_owed(&engine, &never_created),
        "case \"never created\": no record is stored under {never_created:?}, so nothing can be \
         owed"
    );
}

/// The scope half of the request rule: a call addressed at one Region's endpoint but signed
/// for another is served for the Region in its credential scope, which is the seam the
/// Region-rule suites drive.
#[tokio::test]
async fn a_request_is_served_for_the_region_its_credential_scope_names() {
    let (router, _engine) = new_router();

    let created = harness::call_with_scope(
        &router,
        REGION,
        OTHER_REGION,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(OTHER_REGION) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case \"scope Region differs from the host\": expected 202, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    assert!(
        arn.contains(&format!(":{OTHER_REGION}:")),
        "case \"scope Region differs from the host\": the ARN must name the scope Region \
         {OTHER_REGION}, got {arn:?}"
    );

    // The host Region's own inventory is untouched: the record was filed under the scope.
    let host_page = harness::call(&router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        host_page.count("Connectors"),
        0,
        "case \"scope Region differs from the host\": {REGION} must hold no record, got {:?}",
        host_page.body
    );
    let scope_page = harness::call(&router, OTHER_REGION, "GET", "/connectors", None).await;
    assert_eq!(
        scope_page.count("Connectors"),
        1,
        "case \"scope Region differs from the host\": {OTHER_REGION} must hold the record, got \
         {:?}",
        scope_page.body
    );
}
