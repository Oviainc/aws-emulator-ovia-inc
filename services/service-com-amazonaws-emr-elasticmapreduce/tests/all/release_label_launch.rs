// SPDX-License-Identifier: CC0-1.0

//! Launching a cluster resolves its `ReleaseLabel` against the release catalog.
//!
//! THE RULE: a cluster runs ONE Amazon EMR release, so `RunJobFlow` resolves the
//! `ReleaseLabel` it is given against the release catalog this service publishes — the same
//! catalog `ListReleaseLabels` reports and `DescribeReleaseLabel` describes one entry of —
//! and refuses a label naming none of its releases with HTTP 400 `ValidationException`
//! carrying `The supplied release label is invalid: <label>.`, writing no cluster. The
//! member is optional ("for Amazon EMR releases 4.0 and later … use `AmiVersion` for
//! releases 3.x and 2.x"), so a launch naming no release label names no release to resolve
//! and is accepted.
//!
//! The rows span what the rule generalises over: whether the catalog offers the label, and
//! HOW an unusable one misses it — a patch release of a line the catalog has, a line it does
//! not have, the version without the `emr-` prefix, a differently cased prefix, a label with
//! no patch component, the empty string, and no member at all. The accepted rows are read
//! out of the catalog itself rather than written here, so they follow the catalog if it
//! grows, and include its oldest and newest entries, which no part of this issue cited.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// A `RunJobFlow` request naming `release_label` when there is one, under the cluster name
/// `name`.
fn launch(name: &str, release_label: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
    });
    if let Some(label) = release_label {
        body["ReleaseLabel"] = json!(label);
    }
    body
}

/// Every release label the service publishes, read through `ListReleaseLabels` — the
/// catalog as a caller finds it, newest release first.
async fn published_labels(emr: &Emr) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(next) = token.as_ref() {
            body["NextToken"] = json!(next);
        }
        let payload = emr.ok("ListReleaseLabels", body).await;
        let page = at(&payload, &["ReleaseLabels"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("ListReleaseLabels must answer a ReleaseLabels list, got {payload}")
            });
        labels.extend(page.iter().filter_map(|label| {
            label.as_str().map(str::to_string)
        }));
        match at(&payload, &["NextToken"]).as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        labels.len() > 2,
        "the catalog must publish more than two releases, got {labels:?}"
    );
    labels
}

/// The cluster ids `ListClusters` reports.
async fn listed_clusters(emr: &Emr) -> Vec<String> {
    let payload = emr.ok("ListClusters", json!({})).await;
    at(&payload, &["Clusters"])
        .as_array()
        .map(|clusters| {
            clusters
                .iter()
                .filter_map(|cluster| cluster["Id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_launch_is_served_only_for_a_release_the_catalog_offers() {
    let emr = Emr::new();
    let published = published_labels(&emr).await;

    // ACCEPTED: the catalog's oldest and newest releases, and one from the middle of it,
    // taken out of the catalog rather than written here. A launch that names no release
    // label at all is accepted too: the release is then the one `AmiVersion` selects.
    let mut accepted: Vec<Option<String>> = vec![
        Some(published[0].clone()),
        Some(published[published.len() / 2].clone()),
        published.last().cloned(),
        None,
    ];
    accepted.dedup();
    for (index, label) in accepted.iter().enumerate() {
        let case = format!("accepted {label:?}");
        let before = listed_clusters(&emr).await;
        let name = format!("served-{index}");
        let (status, payload) = emr.call("RunJobFlow", launch(&name, label.as_deref())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a release the catalog offers must be served, got {status} \
             with {payload}"
        );
        let cluster_id = payload["JobFlowId"].as_str().unwrap_or_default().to_string();
        assert!(
            !cluster_id.is_empty(),
            "case {case:?}: an accepted launch must answer a JobFlowId, got {payload}"
        );
        let after = listed_clusters(&emr).await;
        assert_eq!(
            after.len(),
            before.len() + 1,
            "case {case:?}: an accepted launch must leave one more cluster, got {after:?}"
        );

        // The cluster reports back the release it was launched with, so the accepted
        // value was persisted rather than only let through.
        let described = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            described["ReleaseLabel"].as_str().map(str::to_string),
            label.clone(),
            "case {case:?}: the cluster must report the release it was launched with, \
             got {described}"
        );
    }

    // REFUSED: every way a label can name none of the catalog's releases.
    let unusable: Vec<String> = vec![
        // A patch release of a line the catalog has, which it does not publish.
        "emr-6.15.99".to_string(),
        // A line the catalog does not have at all.
        "emr-99.99.0".to_string(),
        // One release past the newest the catalog publishes, read off the catalog.
        next_patch(&published[0]),
        // The version without the prefix every label carries.
        published[0]
            .strip_prefix("emr-")
            .expect("a published label carries the emr- prefix")
            .to_string(),
        // The prefix in another case: a label is spelled exactly one way.
        published[0].to_uppercase(),
        // A line rather than a patch release.
        "emr-7.5".to_string(),
        // `XmlStringMaxLen256` admits the empty string, which names no release.
        String::new(),
    ];
    for label in &unusable {
        let case = format!("refused {label:?}");
        let before = listed_clusters(&emr).await;
        let (status, payload) = emr.call("RunJobFlow", launch("refused", Some(label))).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a release the catalog does not offer must be refused 400, \
             got {status} with {payload}"
        );
        let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
        assert!(
            code.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?} in {payload}"
        );
        assert_eq!(
            at(&payload, &["message"])
                .as_str()
                .or_else(|| at(&payload, &["Message"]).as_str()),
            Some(format!("The supplied release label is invalid: {label}.").as_str()),
            "case {case:?}: expected the invalid-release-label sentence, got {payload}"
        );
        assert!(
            payload.get("JobFlowId").is_none(),
            "case {case:?}: a refused launch hands back no cluster id, got {payload}"
        );
        assert_eq!(
            listed_clusters(&emr).await,
            before,
            "case {case:?}: a refused launch must write no cluster"
        );
    }
}

/// The label one patch release past `label`, which the catalog therefore does not publish.
fn next_patch(label: &str) -> String {
    let version = label
        .strip_prefix("emr-")
        .expect("a published label carries the emr- prefix");
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or_default();
    let minor = parts.next().unwrap_or_default();
    let patch: u32 = parts
        .next()
        .and_then(|patch| patch.parse().ok())
        .expect("a published label carries a numeric patch component");
    format!("emr-{major}.{minor}.{}", patch + 1)
}
