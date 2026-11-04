// SPDX-License-Identifier: CC0-1.0

//! The WORDING of two refusals this service composes itself, driven end to end over
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! An error's message is part of its contract: a client branches on it and a recorded
//! expectation matches it, so each of these is asserted as a whole sentence rather
//! than by a substring.
//!
//! * THE TAG PLANE. A well-formed resource ARN of this service that no stored
//!   resource owns is `NotFoundException` (404) with ONE FIXED sentence, shared by the
//!   read, the write and the delete path, which names nothing the request carried — in
//!   particular not the ARN it was handed. AWS's sentence is written out in the
//!   assertion rather than imported, and `state::TAG_RESOURCE_NOT_FOUND_MESSAGE` is
//!   checked against that same text, so neither the handlers nor the constant can
//!   drift from it while this suite is green. Rows differ along what makes the ARN
//!   name nothing (a cluster never created, a cluster created and deleted, a resource
//!   kind this slice does not hold, another Region, another partition, empty
//!   Region/account fields) and along the operation, so the rule is shown to hold per
//!   ARN class on all three.
//! * `DescribeAddonConfiguration`. A `(addonName, addonVersion)` pair the add-on
//!   catalog publishes no build of is `ResourceNotFoundException` (404) stating the
//!   SCHEMA that was not found, with the pair taken from the request's own query
//!   string. The expected sentence is composed in the test from AWS's own fixed text
//!   and the row's values, and `state::addon_schema_not_found_message` is checked
//!   against it, for the same reason. Rows differ along which half of the pair misses,
//!   the form of the value supplied, and the characters it carries — including values
//!   that must travel through the query string unchanged.
//!
//! The catalog values the second suite builds its rows around are read back from
//! `DescribeAddonVersions` rather than written here, so the rows are a published
//! add-on's own name and version whatever the catalog holds.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    addon_schema_not_found_message, TAG_RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_versions::{
    addons, describe_addon_configuration, describe_addon_versions, string_member, versions_of,
};
use crate::cluster_lifecycle::{create, delete, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The three tag operations, each driven at one `resourceArn`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        "",
    )
    .await
}

async fn tag(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        &serde_json::json!({ "tags": { "owner": "platform" } }).to_string(),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/tags/{}?tagKeys=owner", encode_label(arn)),
        "",
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: the tag plane states a resource it did not resolve in one FIXED
// sentence, on all three of its operations, and names nothing from the request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tag_operation_states_an_unresolved_resource_in_the_same_fixed_sentence() {
    let router = emulator();

    // A cluster created and then deleted: its ARN is well formed, was real, and is
    // owned by nothing now.
    let gone = create(&router, &minimal_request("wording-gone-soon")).await;
    assert_eq!(gone.status, 200, "setup create: got {gone:?}");
    let gone_arn = gone.body["cluster"]["arn"]
        .as_str()
        .expect("CreateCluster reports the cluster ARN")
        .to_owned();
    let removed = delete(&router, "wording-gone-soon").await;
    assert_eq!(removed.status, 200, "setup delete: got {removed:?}");

    let rows: Vec<(&str, String)> = vec![
        ("the ARN of a deleted cluster", gone_arn),
        (
            "a cluster ARN for a name never created",
            format!("arn:aws:eks:{REGION}:000000000000:cluster/never-created"),
        ),
        (
            "a cluster ARN in another Region",
            "arn:aws:eks:eu-west-1:000000000000:cluster/never-created".to_string(),
        ),
        (
            "a cluster ARN in another partition",
            "arn:aws-cn:eks:cn-north-1:000000000000:cluster/never-created".to_string(),
        ),
        (
            "a cluster ARN whose Region and account fields are empty",
            "arn:aws:eks:::cluster/never-created".to_string(),
        ),
        (
            "a nodegroup ARN, a resource kind this slice does not hold",
            format!("arn:aws:eks:{REGION}:000000000000:nodegroup/c/n/uuid"),
        ),
        (
            "a capability ARN, another kind this slice does not hold",
            format!("arn:aws:eks:{REGION}:000000000000:capability/c/cap"),
        ),
    ];

    for (row, arn) in &rows {
        for (operation, answer) in [
            ("ListTagsForResource", list_tags(&router, arn).await),
            ("TagResource", tag(&router, arn).await),
            ("UntagResource", untag(&router, arn).await),
        ] {
            let case = format!("{row} / {operation}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {answer:?}"
            );
            let actual = answer.message();
            // The sentence AWS words this refusal with, written out here rather than
            // imported, so a change to the service's own constant cannot keep this
            // green; the shared constant is then checked against it once below, which
            // is what ties every other caller of it to this text.
            assert_eq!(
                actual, "Resource was not found",
                "case {case:?}: expected the tag plane's fixed not-found sentence, got \
                 {actual:?}"
            );
            assert!(
                !actual.contains(arn.as_str()),
                "case {case:?}: the refusal must not echo the requested ARN, got {actual:?}"
            );
        }
    }

    assert_eq!(
        TAG_RESOURCE_NOT_FOUND_MESSAGE, "Resource was not found",
        "the constant every tag operation words this refusal with must be AWS's \
         sentence, got {TAG_RESOURCE_NOT_FOUND_MESSAGE:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `DescribeAddonConfiguration` states the SCHEMA it could not find with
// the `(addonName, addonVersion)` pair the request named.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unpublished_addon_build_is_refused_naming_the_pair_the_request_asked_for() {
    let router = emulator();

    // The catalog as the service publishes it: a published add-on and one of its
    // builds, so the rows below are built from values this service really holds
    // rather than from names written out here.
    let catalog = addons(&describe_addon_versions(&router, "").await);
    assert!(
        catalog.len() >= 2,
        "setup: the catalog must publish more than one add-on, got {} entries",
        catalog.len()
    );
    let published_name = string_member(&catalog[0], "addonName");
    let published_version = string_member(&versions_of(&catalog[0])[0], "addonVersion");
    let other_name = string_member(&catalog[1], "addonName");
    let other_version = string_member(&versions_of(&catalog[1])[0], "addonVersion");

    // (case, addonName, addonVersion) — every row names a pair the catalog publishes
    // no build of, differing along WHICH half misses and along the form of the value
    // supplied for it.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an add-on name the catalog does not publish",
            "not-a-published-addon".to_string(),
            published_version.clone(),
        ),
        (
            "a published add-on at a version it has no build for",
            published_name.clone(),
            "v0.0.0-eksbuild.1".to_string(),
        ),
        (
            "a published add-on at another published add-on's version",
            other_name.clone(),
            published_version.clone(),
        ),
        (
            "a published add-on at a version of another form entirely",
            published_name.clone(),
            "latest".to_string(),
        ),
        (
            "both halves unpublished",
            "vendor/not-an-addon".to_string(),
            "0".to_string(),
        ),
        (
            "an add-on name carrying characters a query string must escape",
            "an add-on name?&=#".to_string(),
            other_version.clone(),
        ),
        (
            "a version carrying characters a query string must escape",
            published_name.clone(),
            "v1 2+3/4".to_string(),
        ),
    ];

    for (case, addon_name, addon_version) in &rows {
        let answer = describe_addon_configuration(&router, addon_name, addon_version).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let actual = answer.message();
        // AWS's own sentence, written out here rather than only imported, so a
        // change to the service's template cannot keep this green. The helper is
        // then checked against the same text, which is what ties the handler to it.
        let expected = format!(
            "Schema not found for requested addonName=[{addon_name}] and \
             addonVersion=[{addon_version}]"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the sentence naming the pair whose schema was not \
             found, got {actual:?}"
        );
        assert_eq!(
            addon_schema_not_found_message(addon_name, addon_version),
            expected,
            "case {case:?}: the template every caller composes this refusal with must be \
             AWS's sentence"
        );
    }

    // CONTROL: the rule is about the pair that does NOT resolve — a pair the catalog
    // does publish is still answered with its schema, so the rows above are refused
    // for the pair they name and not because this operation refuses everything.
    let served = describe_addon_configuration(&router, &published_name, &published_version).await;
    assert_eq!(
        served.status, 200,
        "control: a published build must be answered, got {served:?}"
    );
    assert_eq!(
        served.body["addonName"],
        serde_json::json!(published_name),
        "control: the answer must name the add-on asked for, got {:?}",
        served.body
    );
    assert_eq!(
        served.body["addonVersion"],
        serde_json::json!(published_version),
        "control: the answer must name the version asked for, got {:?}",
        served.body
    );
}
