// SPDX-License-Identifier: CC0-1.0

//! Deleting the registry's signing configuration, end to end over the wire.
//!
//! The rule under test: `DeleteSigningConfiguration` removes the one signing
//! configuration the registry holds and reports the record it removed beside the
//! registry's own account id, so the configuration is gone for every later reader; a
//! delete with nothing stored — none ever put, or a second delete — is refused with
//! `SigningConfigurationNotFoundException` rather than succeeding, and the delete touches
//! no other record ("Deleting the signing configuration does not affect existing image
//! signatures").
//!
//! Every case goes through the router via [`crate::wire`], so it exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, given_layer, ok, REGION, REGISTRY_ID};

/// The only value `SigningRepositoryFilterType` admits
/// (`smithy_query(query="shape",shape="SigningRepositoryFilterType")` → `["WILDCARD_MATCH"]`).
const WILDCARD_MATCH: &str = "WILDCARD_MATCH";

/// `SigningRuleList`'s modelled maximum (`length(min: 0, max: 10)`) — "Maximum of 10 rules".
const MAX_RULES: usize = 10;

/// `SigningRepositoryFilterList`'s modelled maximum (`length(min: 1, max: 100)`).
const MAX_FILTERS: usize = 100;

/// A signing-profile ARN for `name`, built to `SigningProfileArn`'s modelled pattern
/// `^arn:aws(-[a-z]+)*:signer:[a-z0-9-]+:[0-9]{12}:\/signing-profiles\/[a-zA-Z0-9_]{2,}$`.
fn profile_arn(name: &str) -> String {
    format!("arn:aws:signer:{REGION}:{REGISTRY_ID}:/signing-profiles/{name}")
}

/// A rule naming `profile`, carrying one `WILDCARD_MATCH` filter per entry of `filters`
/// (and no `repositoryFilters` member at all when there are none).
fn rule(profile: &str, filters: &[&str]) -> Value {
    let mut rule = json!({"signingProfileArn": profile_arn(profile)});
    if !filters.is_empty() {
        rule["repositoryFilters"] = Value::Array(
            filters
                .iter()
                .map(|filter| json!({"filter": filter, "filterType": WILDCARD_MATCH}))
                .collect(),
        );
    }
    rule
}

/// The body a put sends for `rules`.
fn put_body(rules: &Value) -> Value {
    json!({"signingConfiguration": {"rules": rules}})
}

/// The configurations a delete must hand back. The rows differ along what the rule
/// generalises over: how many rules (none — the modelled minimum —, one, two, the
/// modelled maximum), whether a rule carries repository filters and how many (none, one,
/// several, the modelled maximum), the shape of the filter values, and the partition and
/// profile-name length of the signing-profile ARN.
fn configuration_cases() -> Vec<(&'static str, Value)> {
    let ten_rules: Vec<Value> = (0..MAX_RULES)
        .map(|index| rule(&format!("profile_{index}"), &[]))
        .collect();
    let max_filters: Vec<String> = (0..MAX_FILTERS)
        .map(|index| format!("team-{index}/app"))
        .collect();
    let max_filter_refs: Vec<&str> = max_filters.iter().map(String::as_str).collect();

    vec![
        (
            "no rules — the minimum the modelled length admits",
            json!([]),
        ),
        (
            "one rule, every repository signed by it",
            json!([rule("release_profile", &[])]),
        ),
        (
            "one rule limited to one wildcard filter",
            json!([rule("release_profile", &["prod/*"])]),
        ),
        (
            "one rule whose filters span the filter alphabet",
            json!([rule(
                "ReleaseProfile_01",
                &["*", "prod/*", "team.a_b-c/inner1", "my-app"],
            )]),
        ),
        (
            "two rules, one filtered and one not",
            json!([
                rule("prod_profile", &["prod/*"]),
                rule("catch_all_profile", &[]),
            ]),
        ),
        (
            "a profile name of the shortest length the pattern admits, in another partition",
            json!([json!({
                "signingProfileArn":
                    format!("arn:aws-us-gov:signer:us-gov-west-1:{REGISTRY_ID}:/signing-profiles/ab"),
            })]),
        ),
        (
            "one rule carrying the maximum number of repository filters",
            json!([rule("wide_profile", &max_filter_refs)]),
        ),
        ("the maximum number of rules", Value::Array(ten_rules)),
    ]
}

/// A reader of the record with nothing stored must be refused, not answered with an empty
/// success. Both `GetSigningConfiguration` and `DeleteSigningConfiguration` report that
/// one state, so the same assertion covers either.
async fn assert_not_found(router: &MultiServiceRouter, case: &str, operation: &str) {
    let answer = call(router, operation, &json!({})).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 from {operation} with no configuration stored, \
         got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "SigningConfigurationNotFoundException",
        "case {case:?}: expected SigningConfigurationNotFoundException from {operation}, \
         got {:?}",
        answer.body
    );
}

/// Whatever shape of configuration the registry holds, the delete hands that record back
/// and the registry then holds none: the get and a second delete are both refused. Each
/// row runs on its own emulator, so the before-and-after refusals are about that row.
#[tokio::test]
async fn a_delete_removes_the_stored_configuration_and_reports_it() {
    for (case, rules) in configuration_cases() {
        let router = emulator();
        let expected = json!({"rules": rules});

        // Nothing stored yet: the delete has nothing to remove.
        assert_not_found(&router, case, "DeleteSigningConfiguration").await;

        ok(&router, case, "PutSigningConfiguration", &put_body(&rules)).await;

        let deleted = ok(&router, case, "DeleteSigningConfiguration", &json!({})).await;
        assert_eq!(
            deleted["signingConfiguration"], expected,
            "case {case:?}: the delete must report the configuration it removed \
             {expected:?}, got {:?}",
            deleted["signingConfiguration"]
        );
        assert_eq!(
            deleted["registryId"],
            json!(REGISTRY_ID),
            "case {case:?}: expected registryId {REGISTRY_ID}, got {:?}",
            deleted["registryId"]
        );

        // The record is gone for every reader, and the delete is not idempotent.
        assert_not_found(&router, case, "GetSigningConfiguration").await;
        assert_not_found(&router, case, "DeleteSigningConfiguration").await;
    }
}

/// The delete removes the record the LAST put stored, and a put after a delete makes the
/// registry hold a configuration again — so the registry goes from holding one to holding
/// none and back for every row of the table.
#[tokio::test]
async fn a_put_after_a_delete_restores_the_registrys_configuration() {
    let router = emulator();

    for (case, rules) in configuration_cases() {
        let expected = json!({"rules": rules});
        ok(&router, case, "PutSigningConfiguration", &put_body(&rules)).await;

        let got = ok(&router, case, "GetSigningConfiguration", &json!({})).await;
        assert_eq!(
            got["signingConfiguration"], expected,
            "case {case:?}: the get must report the configuration of the last put \
             {expected:?}, got {:?}",
            got["signingConfiguration"]
        );

        let deleted = ok(&router, case, "DeleteSigningConfiguration", &json!({})).await;
        assert_eq!(
            deleted["signingConfiguration"], expected,
            "case {case:?}: the delete must remove the configuration the last put stored \
             {expected:?}, got {:?}",
            deleted["signingConfiguration"]
        );
        assert_not_found(&router, case, "GetSigningConfiguration").await;
    }
}

/// "Deleting the signing configuration does not affect existing image signatures": the
/// delete removes the registry's one configuration record and nothing else, so the
/// repositories and the images in them are still reported afterwards.
#[tokio::test]
async fn deleting_the_configuration_leaves_the_repositories_and_images_alone() {
    let router = emulator();
    let case = "a registry with repositories and an image";

    for name in ["signed-app", "other/app"] {
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": name}),
        )
        .await;
    }
    // The config blob the manifest names, uploaded before the manifest that references
    // it is pushed: a manifest cannot be pushed before its blobs are in the repository.
    let config_digest = given_layer(&router, case, "signed-app", b"signed-app config").await;
    let manifest = format!(
        "{{\"schemaVersion\":2,\
         \"mediaType\":\"application/vnd.docker.distribution.manifest.v2+json\",\
         \"config\":{{\"size\":7023,\"digest\":\"{config_digest}\"}}}}"
    );
    let pushed = ok(
        &router,
        case,
        "PutImage",
        &json!({
            "repositoryName": "signed-app",
            "imageManifest": manifest,
            "imageTag": "v1",
        }),
    )
    .await;
    let digest = pushed["image"]["imageId"]["imageDigest"].clone();

    ok(
        &router,
        case,
        "PutSigningConfiguration",
        &put_body(&json!([rule("release_profile", &["signed-app"])])),
    )
    .await;
    ok(&router, case, "DeleteSigningConfiguration", &json!({})).await;
    assert_not_found(&router, case, "GetSigningConfiguration").await;

    let described = ok(
        &router,
        case,
        "DescribeRepositories",
        &json!({"repositoryNames": ["signed-app", "other/app"]}),
    )
    .await;
    let names: Vec<Value> = described["repositories"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeRepositories must return a list"))
        .iter()
        .map(|repository| repository["repositoryName"].clone())
        .collect();
    assert_eq!(
        names,
        vec![json!("other/app"), json!("signed-app")],
        "case {case:?}: deleting the signing configuration must leave the repositories, \
         got {names:?}"
    );

    let images = ok(
        &router,
        case,
        "DescribeImages",
        &json!({"repositoryName": "signed-app"}),
    )
    .await;
    assert_eq!(
        images["imageDetails"][0]["imageDigest"], digest,
        "case {case:?}: the pushed image must survive the delete, expected {digest:?}, \
         got {:?}",
        images["imageDetails"][0]["imageDigest"]
    );
}
