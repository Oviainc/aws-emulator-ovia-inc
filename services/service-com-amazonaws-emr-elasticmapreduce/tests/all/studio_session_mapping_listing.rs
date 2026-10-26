// SPDX-License-Identifier: CC0-1.0

//! `ListStudioSessionMappings`, end to end over the wire.
//!
//! THE RULE every case below asserts: the listing reports the persisted session mappings
//! that match the request's filters — the `StudioId` it names, or every Studio when it
//! names none, narrowed to one `IdentityType` when it filters on one — in ONE stable
//! order, at most a page at a time, and a walk driven by the `Marker` it answers visits
//! every matching mapping exactly once and then stops. Repeating a walk reports the same
//! mappings in the same order, because a listing only reports: it is a projection of what
//! `CreateStudioSessionMapping` stored, `UpdateStudioSessionMapping` changed and
//! `DeleteStudioSessionMapping` removed.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the harness
//! `tests/all/job_flow_lifecycle.rs` owns. Nothing reaches into the service's state
//! struct: an entry of a page is only ever compared with what `GetStudioSessionMapping`
//! answers for the same mapping.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::LIST_PAGE_SIZE;

use crate::emr_control_plane::studio_request;
use crate::job_flow_lifecycle::{assert_refused, at, Emr};

/// Every value `com.amazonaws.emr#IdentityType` declares
/// (`smithy_query(query="shape", shape="IdentityType")`).
const IDENTITY_TYPES: [&str; 2] = ["USER", "GROUP"];

/// The session policies the mappings under test carry, so the policy of one entry is not
/// the policy of every entry.
const SESSION_POLICIES: [&str; 3] = [
    "arn:aws:iam::aws:policy/EMRStudioBasic",
    "arn:aws:iam::aws:policy/EMRStudioIntermediate",
    "arn:aws:iam::aws:policy/EMRStudioAdvanced",
];

/// The members a `SessionMappingSummary` carries
/// (`smithy_query(query="shape", shape="SessionMappingSummary")`).
const SUMMARY_MEMBERS: [&str; 6] = [
    "StudioId",
    "IdentityId",
    "IdentityName",
    "IdentityType",
    "SessionPolicyArn",
    "CreationTime",
];

/// One mapping this suite created, as it was created: enough to address it again and to
/// say what the listing must report for it.
#[derive(Clone, Debug)]
struct Mapping {
    studio_id: String,
    /// Which member named the identity: "Either `IdentityName` or `IdentityId` must be
    /// specified, but not both" (`CreateStudioSessionMapping$IdentityId` doc), so the
    /// rows span both ways of naming one.
    identity_member: &'static str,
    identity: String,
    identity_type: &'static str,
    session_policy_arn: String,
}

impl Mapping {
    fn create_body(&self) -> Value {
        let mut body = self.lookup();
        body["SessionPolicyArn"] = json!(&self.session_policy_arn);
        body
    }

    /// The request that addresses this mapping: its Studio, its identity type and the
    /// one member that names its identity.
    fn lookup(&self) -> Value {
        json!({
            "StudioId": &self.studio_id,
            "IdentityType": self.identity_type,
            self.identity_member: &self.identity,
        })
    }

    /// What identifies this mapping, as both a created row and a reported entry can say
    /// it.
    fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.studio_id, self.identity_type, self.identity
        )
    }
}

/// [`Mapping::key`] for one entry of a reported page.
fn row_key(row: &Value) -> String {
    let identity = row["IdentityId"]
        .as_str()
        .or_else(|| row["IdentityName"].as_str())
        .unwrap_or_default();
    format!(
        "{}|{}|{identity}",
        row["StudioId"].as_str().unwrap_or_default(),
        row["IdentityType"].as_str().unwrap_or_default()
    )
}

/// The keys of every entry of every page, in the order the walk reported them.
fn row_keys(pages: &[Vec<Value>]) -> Vec<String> {
    pages.iter().flatten().map(row_key).collect()
}

/// Walk every page of a `ListStudioSessionMappings` request, answering the pages it
/// reported.
///
/// Asserts the page invariants on the way: a page carries at most `LIST_PAGE_SIZE`
/// entries, and a page that answers a marker is FULL — so the marker means "there is
/// more" and never "that page was short".
async fn walk(emr: &Emr, case: &str, request: &Value) -> Vec<Vec<Value>> {
    let mut pages: Vec<Vec<Value>> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(marker) = &marker {
            body["Marker"] = json!(marker);
        }
        let out = emr.ok("ListStudioSessionMappings", body).await;
        let rows = at(&out, &["SessionMappings"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: SessionMappings must be a list, got {out}"));
        assert!(
            rows.len() <= LIST_PAGE_SIZE,
            "case {case:?}: a page carries at most {LIST_PAGE_SIZE} entries, got {}",
            rows.len()
        );
        marker = out.get("Marker").and_then(Value::as_str).map(str::to_string);
        if marker.is_some() {
            assert_eq!(
                rows.len(),
                LIST_PAGE_SIZE,
                "case {case:?}: a page that answers a marker must be full, got {}",
                rows.len()
            );
        }
        pages.push(rows);
        if marker.is_none() {
            return pages;
        }
        assert!(
            pages.len() <= 16,
            "case {case:?}: the walk must end, got {} pages",
            pages.len()
        );
    }
}

/// The listing reports every persisted mapping its filters match, exactly once, a page at
/// a time, in the same order on every walk.
///
/// The rows differ along what the listing generalises over: how many mappings a Studio
/// holds (none, part of a page, more than two full pages), which Studio is asked for (one
/// of them, or every one), which `IdentityType` the request filters on, and which member
/// named the identity.
#[tokio::test]
async fn the_mapping_listing_pages_every_persisted_mapping_in_one_stable_order() {
    let emr = Emr::new();

    let mut studios: Vec<String> = Vec::new();
    for name in ["busy studio", "small studio", "empty studio"] {
        let out = emr.ok("CreateStudio", studio_request(name)).await;
        studios.push(
            out["StudioId"]
                .as_str()
                .unwrap_or_else(|| panic!("CreateStudio must answer a StudioId, got {out}"))
                .to_string(),
        );
    }

    // The counts span the page bound: more than two full pages for the first Studio, a
    // part page for the second, nothing at all for the third.
    let counts = [2 * LIST_PAGE_SIZE + 3, 7, 0];
    let mut created: Vec<Mapping> = Vec::new();
    for (studio_id, count) in studios.iter().zip(counts) {
        for index in 0..count {
            let by_id = index % 3 == 0;
            let mapping = Mapping {
                studio_id: studio_id.clone(),
                identity_member: if by_id { "IdentityId" } else { "IdentityName" },
                identity: if by_id {
                    format!("{index:08x}-3df9-4e8a-9f0a-example")
                } else {
                    format!("identity-{index:04}")
                },
                identity_type: IDENTITY_TYPES[index % IDENTITY_TYPES.len()],
                session_policy_arn: SESSION_POLICIES[index % SESSION_POLICIES.len()].to_string(),
            };
            emr.ok("CreateStudioSessionMapping", mapping.create_body())
                .await;
            created.push(mapping);
        }
    }

    // (case, the Studio the request names, the IdentityType it filters on)
    let cases: [(&str, Option<usize>, Option<&str>); 9] = [
        ("the busy studio, both identity types", Some(0), None),
        ("the busy studio, users only", Some(0), Some("USER")),
        ("the busy studio, groups only", Some(0), Some("GROUP")),
        ("the small studio, both identity types", Some(1), None),
        ("the small studio, groups only", Some(1), Some("GROUP")),
        ("the empty studio", Some(2), None),
        ("every studio, both identity types", None, None),
        ("every studio, users only", None, Some("USER")),
        ("every studio, groups only", None, Some("GROUP")),
    ];

    for (case, studio_index, identity_type) in cases {
        let mut request = json!({});
        if let Some(index) = studio_index {
            request["StudioId"] = json!(&studios[index]);
        }
        if let Some(wanted) = identity_type {
            request["IdentityType"] = json!(wanted);
        }

        let mut expected: Vec<String> = created
            .iter()
            .filter(|mapping| {
                studio_index.is_none_or(|index| mapping.studio_id == studios[index])
            })
            .filter(|mapping| identity_type.is_none_or(|wanted| mapping.identity_type == wanted))
            .map(Mapping::key)
            .collect();
        expected.sort();

        let pages = walk(&emr, case, &request).await;
        // A walk is as many pages as the matching mappings fill — never an extra empty
        // one — and an account that matches nothing still answers one empty page.
        let expected_pages = expected.len().div_ceil(LIST_PAGE_SIZE).max(1);
        assert_eq!(
            pages.len(),
            expected_pages,
            "case {case:?}: expected {expected_pages} page(s) for {} mapping(s), got {}",
            expected.len(),
            pages.len()
        );

        let reported = row_keys(&pages);
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case:?}: every mapping is reported once, got {reported:?}"
        );
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            sorted, expected,
            "case {case:?}: expected {expected:?}, got {sorted:?}"
        );

        // The same walk, repeated, reports the same mappings in the same order: the
        // ordering is the store's and the listing changed nothing.
        let again = walk(&emr, case, &request).await;
        assert_eq!(
            row_keys(&again),
            reported,
            "case {case:?}: a repeated walk must report the same order"
        );
    }

    // Every entry is the summary the model declares, carrying what the stored mapping
    // carries and not the detail member the summary leaves out.
    let busy = json!({ "StudioId": &studios[0] });
    let pages = walk(&emr, "the busy studio", &busy).await;
    for row in pages.iter().flatten() {
        let key = row_key(row);
        let mapping = created
            .iter()
            .find(|mapping| mapping.key() == key)
            .unwrap_or_else(|| panic!("case {key:?}: an entry no mapping was created for"));
        let detail = emr.ok("GetStudioSessionMapping", mapping.lookup()).await
            ["SessionMapping"]
            .clone();
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                row.get(member),
                detail.get(member),
                "case {key:?}: {member} must be the stored one, got {row}"
            );
        }
        assert_eq!(
            row["SessionPolicyArn"].as_str(),
            Some(mapping.session_policy_arn.as_str()),
            "case {key:?}: the session policy must be the created one, got {row}"
        );
        assert!(
            row.get("LastModifiedTime").is_none(),
            "case {key:?}: a summary carries no LastModifiedTime, got {row}"
        );
    }

    // The marker a page answers resumes the walk at the first mapping that page did not
    // report, so the pages of a walk are a partition of the matching mappings.
    let whole_walk = row_keys(&pages);
    let first_page = emr.ok("ListStudioSessionMappings", busy.clone()).await;
    let marker = first_page["Marker"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of {} entries answers a marker, got {first_page}", LIST_PAGE_SIZE))
        .to_string();
    let mut resumed = busy.clone();
    resumed["Marker"] = json!(&marker);
    let resumed = emr.ok("ListStudioSessionMappings", resumed).await;
    let resumed_keys = row_keys(&[at(&resumed, &["SessionMappings"])
        .as_array()
        .cloned()
        .unwrap_or_default()]);
    assert_eq!(
        resumed_keys.first().map(String::as_str),
        whole_walk.get(LIST_PAGE_SIZE).map(String::as_str),
        "a resumed page starts at the first mapping the page before it did not report, got {resumed}"
    );

    // An update and a delete are both reported, so a page is read from the store and not
    // from a snapshot taken when the mapping was created.
    let mut updated = created[1].clone();
    updated.session_policy_arn = SESSION_POLICIES[2].to_string();
    emr.ok("UpdateStudioSessionMapping", updated.create_body())
        .await;
    let removed = created[0].clone();
    emr.ok("DeleteStudioSessionMapping", removed.lookup()).await;

    let after = row_keys(&walk(&emr, "after the update and the delete", &busy).await);
    assert_eq!(
        after.len(),
        whole_walk.len() - 1,
        "a deleted mapping leaves the listing, got {} entries",
        after.len()
    );
    assert!(
        !after.contains(&removed.key()),
        "the deleted mapping must be gone, got {after:?}"
    );
    let pages = walk(&emr, "the updated mapping", &busy).await;
    let entry = pages
        .iter()
        .flatten()
        .find(|row| row_key(row) == updated.key())
        .unwrap_or_else(|| panic!("case {:?}: the updated mapping must be listed", updated.key()));
    assert_eq!(
        entry["SessionPolicyArn"].as_str(),
        Some(updated.session_policy_arn.as_str()),
        "the listing must report the updated session policy, got {entry}"
    );

    // A marker no mapping of THIS listing is keyed by is refused rather than read as a
    // resume point: the empty string, a value that is not a marker at all, and a marker
    // that another listing — one whose matching mappings this one does not hold —
    // answered.
    for (case, studio_index, marker) in [
        ("an empty marker", 0, String::new()),
        ("a marker no mapping is keyed by", 0, "not-a-marker".to_string()),
        ("another listing's marker", 2, marker),
    ] {
        let (status, payload) = emr
            .call(
                "ListStudioSessionMappings",
                json!({ "StudioId": &studios[studio_index], "Marker": marker }),
            )
            .await;
        assert_refused(case, status, &payload, "InvalidRequestException");
    }
}
