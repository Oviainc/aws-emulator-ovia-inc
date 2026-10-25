// SPDX-License-Identifier: CC0-1.0

//! The rule the `assetModelVersion` selector follows on every asset-model read
//! that carries one, driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with.
//!
//! A read naming a version alias is answered from THAT version of the asset
//! model: `LATEST` (the documented default) is the definition the newest
//! accepted write left, and `ACTIVE` is the newest definition that has reached
//! the `ACTIVE` state — which, while a write the model already accepted is
//! still settling, is the definition that write superseded. A model no version
//! of which has ever been active has no `ACTIVE` version, and a read naming it
//! is refused `404 ResourceNotFoundException`.
//!
//! The selector's catalogue is the modelled one: `AssetModelVersionFilter`
//! carries the pattern `^(LATEST|ACTIVE)$`
//! (`smithy_query(query="shape", shape="AssetModelVersionFilter")`), and the
//! member's own documentation states the default — "The version alias that
//! specifies the latest or active version of the asset model. The details are
//! returned in the response. The default value is LATEST."
//!
//! The rows span the three reads that carry the selector, each of the three
//! forms the selector arrives in (absent, `LATEST`, `ACTIVE`), the writes that
//! open a settling window, and both sides of that window.
//!
//! The wire harness (`router`, `call`, `create_model`, the error readers and the
//! activation wait) lives in the `hierarchies_etags_and_refusals` suite, which
//! this file reuses rather than keeping a copy of.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::hierarchies_etags_and_refusals::{
    call, call_with_headers, create_model, error_code, error_message, router, PAST_ACTIVATION,
};

/// The selector value naming the newest definition — the documented default.
const LATEST: &str = "LATEST";

/// The selector value naming the newest definition that has become active.
const ACTIVE: &str = "ACTIVE";

/// The name of the composite model every model in this suite is seeded with, so
/// a read of one composite model addresses the same composite model in both
/// versions.
const COMPOSITE_NAME: &str = "Alarm";

/// The composite model's id, chosen so a read can address it before anything is
/// described.
const COMPOSITE_ID: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

/// One of the reads that carry `assetModelVersion`.
#[derive(Clone, Copy, Debug)]
enum Read {
    /// `DescribeAssetModel`.
    Model,
    /// `ListAssetModelProperties` under the `ALL` filter, so it reaches the
    /// composite model's definitions too.
    Properties,
    /// `DescribeAssetModelCompositeModel` on the seeded composite model.
    CompositeModel,
}

/// Every read that carries the selector, so a rule stated once is asserted of
/// all of them.
const READS: [Read; 3] = [Read::Model, Read::Properties, Read::CompositeModel];

impl Read {
    /// Where this read is bound, for `model_id`'s version `version` — `None`
    /// being the selector left off entirely.
    fn uri(self, model_id: &str, version: Option<&str>) -> String {
        match self {
            Self::Model => format!("/asset-models/{model_id}{}", first_param(version)),
            Self::Properties => format!(
                "/asset-models/{model_id}/properties?filter=ALL&maxResults=250{}",
                next_param(version)
            ),
            Self::CompositeModel => format!(
                "/asset-models/{model_id}/composite-models/{COMPOSITE_ID}{}",
                first_param(version)
            ),
        }
    }

    /// The property names this read reports, in the order it reports them: the
    /// one member whose value differs between the two versions of the models
    /// this suite writes.
    fn names(self, body: &Value) -> Vec<String> {
        let list = match self {
            Self::Model => &body["assetModelProperties"],
            Self::Properties => &body["assetModelPropertySummaries"],
            Self::CompositeModel => &body["assetModelCompositeModelProperties"],
        };
        list.as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The property names this read reports of the SEEDED definition — the one
    /// the write in `each_read_answers_from_the_version_its_selector_names`
    /// supersedes.
    fn before_names(self) -> Vec<String> {
        match self {
            Self::Model => vec!["Temperature".to_string()],
            Self::Properties => vec!["Temperature".to_string(), "Threshold".to_string()],
            Self::CompositeModel => vec!["Threshold".to_string()],
        }
    }

    /// The property names this read reports of the definition that write left.
    fn after_names(self) -> Vec<String> {
        match self {
            Self::Model => vec!["Pressure".to_string()],
            Self::Properties => vec!["Pressure".to_string(), "Limit".to_string()],
            Self::CompositeModel => vec!["Limit".to_string()],
        }
    }
}

/// The selector as the FIRST query parameter of a URI that has no others.
fn first_param(version: Option<&str>) -> String {
    version
        .map(|version| format!("?assetModelVersion={version}"))
        .unwrap_or_default()
}

/// The selector appended to a URI that already carries query parameters.
fn next_param(version: Option<&str>) -> String {
    version
        .map(|version| format!("&assetModelVersion={version}"))
        .unwrap_or_default()
}

/// One property definition: `name` as a `DOUBLE` measurement, which every row
/// here can hold since a write may not change a kept property's data type.
fn measurement(name: &str) -> Value {
    json!({"name": name, "dataType": "DOUBLE", "type": {"measurement": {}}})
}

/// The definition body both the create and the update carry: a model named
/// `name` with the base property `base` and one composite model of its own
/// holding `nested`.
///
/// One builder for both, since `CreateAssetModel` and `UpdateAssetModel` state
/// an asset model's definition in the same members: the create seeds the
/// definition a write then supersedes, and the update replaces its two property
/// lists while keeping the composite model they are defined on.
fn definition_body(name: &str, base: &str, nested: &str) -> Value {
    json!({
        "assetModelName": name,
        "assetModelProperties": [measurement(base)],
        "assetModelCompositeModels": [{
            "id": COMPOSITE_ID,
            "name": COMPOSITE_NAME,
            "type": "CUSTOM",
            "properties": [measurement(nested)],
        }],
    })
}

/// A read of `model_id` at `version`, asserting it answered, as its parsed body.
async fn read(
    router: &MultiServiceRouter,
    which: Read,
    model_id: &str,
    version: Option<&str>,
) -> Value {
    let uri = which.uri(model_id, version);
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {which:?} at {version:?}: expected 200 from {uri}, got {status} {body}"
    );
    body
}

/// `GET /asset-models/{id}` at `version`, asserting it answered.
async fn describe(router: &MultiServiceRouter, model_id: &str, version: Option<&str>) -> Value {
    read(router, Read::Model, model_id, version).await
}

/// The same, also answering the `ETag` response header that read carries: the
/// entity tag is bound to the header, not to a body member.
async fn describe_with_e_tag(
    router: &MultiServiceRouter,
    model_id: &str,
    version: Option<&str>,
) -> (Value, String) {
    let uri = Read::Model.uri(model_id, version);
    let (status, headers, body) = call_with_headers(router, "GET", &uri, None, &[]).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel at {version:?}: expected 200 from {uri}, got {status} {body}"
    );
    let e_tag = headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("DescribeAssetModel at {version:?}: no ETag among {headers:?}"))
        .to_string();
    (body, e_tag)
}

#[tokio::test]
async fn each_read_answers_from_the_version_its_selector_names() {
    let router = router();
    let model_id =
        create_model(&router, definition_body("Boiler", "Temperature", "Threshold")).await;
    // Waited out, so the seeded definition has BECOME the active version: that
    // is what the write below supersedes.
    tokio::time::sleep(PAST_ACTIVATION).await;

    let (before, active_e_tag) = describe_with_e_tag(&router, &model_id, None).await;
    let active_version = before["assetModelVersion"]
        .as_str()
        .expect("the described model carries a version")
        .to_string();

    let (status, answered) = call(
        &router,
        "PUT",
        &format!("/asset-models/{model_id}"),
        Some(definition_body("Boiler", "Pressure", "Limit")),
    )
    .await;
    assert_eq!(status, 202, "UpdateAssetModel: got {status} {answered}");
    assert_eq!(
        answered["assetModelStatus"]["state"].as_str(),
        Some("UPDATING"),
        "the accepted update must report the model as UPDATING, got {answered}"
    );

    // One row per read and per form of the selector, while the update is still
    // settling: the superseded definition is the active one, and the one the
    // update wrote is the latest.
    for which in READS {
        for (case, version, expected) in [
            (
                "the selector left off, which is LATEST",
                None,
                which.after_names(),
            ),
            ("LATEST", Some(LATEST), which.after_names()),
            ("ACTIVE", Some(ACTIVE), which.before_names()),
        ] {
            let body = read(&router, which, &model_id, version).await;
            assert_eq!(
                which.names(&body),
                expected,
                "case {which:?} {case:?}: expected the properties {expected:?}, got {body}"
            );
        }
    }

    // What `DescribeAssetModel` reports ABOUT the version it answered from: its
    // number, its entity tag and the state that version is in.
    let (latest, latest_e_tag) = describe_with_e_tag(&router, &model_id, Some(LATEST)).await;
    let (active, active_read_e_tag) = describe_with_e_tag(&router, &model_id, Some(ACTIVE)).await;
    assert_eq!(
        active["assetModelVersion"].as_str(),
        Some(active_version.as_str()),
        "the ACTIVE read must report the superseded version's number, got {active}"
    );
    assert_ne!(
        latest["assetModelVersion"].as_str(),
        Some(active_version.as_str()),
        "the LATEST read must report a new version number, got {latest}"
    );
    assert_eq!(
        active_read_e_tag, active_e_tag,
        "the ACTIVE read must report the tag that version was read with, got {active}"
    );
    assert_ne!(
        latest_e_tag, active_e_tag,
        "the LATEST read must report the tag of the definition the write left, got {latest}"
    );
    assert_eq!(
        active["assetModelStatus"]["state"].as_str(),
        Some("ACTIVE"),
        "the version selected as the active one is in the ACTIVE state, got {active}"
    );
    assert_eq!(
        latest["assetModelStatus"]["state"].as_str(),
        Some("UPDATING"),
        "the version the write wrote is still settling, got {latest}"
    );
    assert_eq!(
        active["assetModelName"].as_str(),
        before["assetModelName"].as_str(),
        "the ACTIVE read must report the superseded definition's name, got {active}"
    );
    assert_eq!(
        active["assetModelId"].as_str(),
        Some(model_id.as_str()),
        "both versions are versions of ONE model, got {active}"
    );

    // Reading a version changes nothing a later caller sees: the latest version
    // is still what it was before the ACTIVE reads above.
    for which in READS {
        let body = read(&router, which, &model_id, Some(LATEST)).await;
        assert_eq!(
            which.names(&body),
            which.after_names(),
            "case {which:?} \"LATEST after the ACTIVE reads\": expected {:?}, got {body}",
            which.after_names()
        );
    }

    // Once the transition the write owes falls due, the version it wrote IS the
    // active one, so both selectors name one definition again.
    tokio::time::sleep(PAST_ACTIVATION).await;
    for which in READS {
        for (case, version) in [("LATEST", Some(LATEST)), ("ACTIVE", Some(ACTIVE))] {
            let body = read(&router, which, &model_id, version).await;
            assert_eq!(
                which.names(&body),
                which.after_names(),
                "case {which:?} {case:?} once the update settled: expected {:?}, got {body}",
                which.after_names()
            );
        }
    }
    let settled = describe(&router, &model_id, Some(ACTIVE)).await;
    assert_eq!(
        settled["assetModelVersion"].as_str(),
        latest["assetModelVersion"].as_str(),
        "once it settles, the active version is the one the write wrote, got {settled}"
    );
}

#[tokio::test]
async fn every_write_that_bumps_the_version_leaves_the_superseded_definition_active() {
    let router = router();

    // One row per write that changes an asset model's definition, so the rule is
    // asserted of the writes as a class rather than of the one that was
    // reported. Each row answers the request that opens its settling window.
    type Write = (&'static str, &'static str, String, Value);
    let rows: Vec<Write> = vec![
        (
            "UpdateAssetModel",
            "PUT",
            String::new(),
            definition_body("Boiler", "Pressure", "Limit"),
        ),
        (
            "CreateAssetModelCompositeModel",
            "POST",
            "/composite-models".to_string(),
            json!({
                "assetModelCompositeModelName": "Second",
                "assetModelCompositeModelType": "CUSTOM",
            }),
        ),
        (
            "DeleteAssetModelCompositeModel",
            "DELETE",
            // The composite model the seed defines, removed from the definition.
            format!("/composite-models/{COMPOSITE_ID}"),
            Value::Null,
        ),
    ];

    // One model per row, created first and waited for ONCE: the owed transition
    // is a clock the whole store shares.
    let mut model_ids = Vec::new();
    for (case, ..) in &rows {
        model_ids.push(
            create_model(&router, definition_body(case, "Temperature", "Threshold")).await,
        );
    }
    tokio::time::sleep(PAST_ACTIVATION).await;

    for ((case, method, suffix, body), model_id) in rows.iter().zip(&model_ids) {
        let (before, before_e_tag) = describe_with_e_tag(&router, model_id, None).await;
        assert_eq!(
            before["assetModelStatus"]["state"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: the seed must have settled before the write, got {before}"
        );

        let request = (*body != Value::Null).then(|| body.clone());
        let (status, answered) = call(
            &router,
            method,
            &format!("/asset-models/{model_id}{suffix}"),
            request,
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected 202, got {status} {answered}"
        );

        // The write superseded the definition that was active, so that
        // definition — every member of it, as the read reports them — is what
        // the ACTIVE selector is still answered from.
        let (active, active_e_tag) = describe_with_e_tag(&router, model_id, Some(ACTIVE)).await;
        assert_eq!(
            active_e_tag, before_e_tag,
            "case {case:?}: the ACTIVE version's entity tag must be the superseded one's, got \
             {active}"
        );
        for member in [
            "assetModelVersion",
            "assetModelName",
            "assetModelProperties",
            "assetModelHierarchies",
            "assetModelCompositeModels",
            "assetModelCompositeModelSummaries",
            "assetModelLastUpdateDate",
        ] {
            assert_eq!(
                active[member], before[member],
                "case {case:?}: the ACTIVE version's {member} must be the superseded one's, got \
                 {active}"
            );
        }

        // And the LATEST selector from the definition the write left, which
        // carries a version of its own and is still settling.
        let latest = describe(&router, model_id, Some(LATEST)).await;
        assert_ne!(
            latest["assetModelVersion"], before["assetModelVersion"],
            "case {case:?}: the write must leave a new version, got {latest}"
        );
        assert_eq!(
            latest["assetModelStatus"]["state"].as_str(),
            Some("UPDATING"),
            "case {case:?}: the version the write wrote is still settling, got {latest}"
        );
        assert_eq!(
            latest, describe(&router, model_id, None).await,
            "case {case:?}: the selector left off must answer exactly as LATEST does, got {latest}"
        );
    }

    // Once the window closes, each model's own newest version is its active one.
    tokio::time::sleep(PAST_ACTIVATION).await;
    for ((case, ..), model_id) in rows.iter().zip(&model_ids) {
        let active = describe(&router, model_id, Some(ACTIVE)).await;
        let latest = describe(&router, model_id, Some(LATEST)).await;
        assert_eq!(
            active, latest,
            "case {case:?}: once the write settled both selectors name one definition, got \
             {active} and {latest}"
        );
    }
}

#[tokio::test]
async fn a_model_with_no_active_version_refuses_the_active_selector() {
    let router = router();
    // NOT waited for: a model that has never settled has a latest version and no
    // active one at all.
    let model_id =
        create_model(&router, definition_body("Boiler", "Temperature", "Threshold")).await;

    for which in READS {
        // The selector left off and LATEST both name the version the create
        // wrote, which exists whatever state it is in.
        for (case, version) in [("the selector left off", None), ("LATEST", Some(LATEST))] {
            let body = read(&router, which, &model_id, version).await;
            assert_eq!(
                which.names(&body),
                which.before_names(),
                "case {which:?} {case:?}: expected {:?}, got {body}",
                which.before_names()
            );
        }

        // ACTIVE names a version this model does not have.
        let uri = which.uri(&model_id, Some(ACTIVE));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {which:?} \"ACTIVE before the model has ever settled\": expected 404 from \
             {uri}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {which:?}: unexpected error code in {body}"
        );
        // Written out by hand from the rule, as an independent oracle: the
        // absence names the model the request named and the state it does
        // report, which is what tells the caller to wait.
        assert_eq!(
            error_message(&body),
            format!("The asset model {model_id} has no ACTIVE version yet; it is CREATING."),
            "case {which:?}: unexpected refusal message in {body}"
        );
    }

    // The create's own owed transition makes that version the active one, so the
    // same requests are answered.
    tokio::time::sleep(PAST_ACTIVATION).await;
    for which in READS {
        let body = read(&router, which, &model_id, Some(ACTIVE)).await;
        assert_eq!(
            which.names(&body),
            which.before_names(),
            "case {which:?} \"ACTIVE once the create settled\": expected {:?}, got {body}",
            which.before_names()
        );
    }
}
