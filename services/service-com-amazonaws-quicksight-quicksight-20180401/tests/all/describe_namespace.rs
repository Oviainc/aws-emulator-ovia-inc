// SPDX-License-Identifier: CC0-1.0

//! `DescribeNamespace`: the report a namespace read answers with, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see.
//!
//! The rule under test: a namespace read reports the record the create persisted for
//! the exact `(AwsAccountId, Namespace)` pair the request addresses — so the record's
//! own values, not the reader's request, are what it reports; a pair no namespace is
//! held under is refused `ResourceNotFoundException`; and, being a report, the read
//! leaves the store exactly as a later caller would have found it.
//!
//! The rows therefore vary what that rule generalises over — the region the create ran
//! in against the region the read is signed for, the name's case and form, how many
//! times the same namespace is read — rather than re-running one recorded call.
//! `namespace_lifecycle` covers the create-then-read round trip itself.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;
use super::namespace_lifecycle::{
    create_namespace_in, describe_namespace_in, namespace_arn_in, settle_creation,
};

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";

/// `CreateNamespace` of a namespace with no tags, signed for `region`.
async fn create_namespace(
    harness: &Harness,
    region: &str,
    account: &str,
    namespace: &str,
) -> (u16, Value) {
    create_namespace_in(
        harness,
        region,
        account,
        json!({ "Namespace": namespace, "IdentityStore": "QUICKSIGHT" }),
    )
    .await
}

/// The namespace names held in one account, as the store holds them — what a case
/// compares before and after a read to show the read persisted nothing.
fn stored_names(harness: &Harness, account: &str) -> Vec<String> {
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    let mut names: Vec<String> = store
        .namespaces
        .keys()
        .filter(|(owner, _)| owner == account)
        .map(|(_, name)| name.clone())
        .collect();
    names.sort();
    names
}

/// One row of the "the record is what is reported" table: a namespace created in one
/// region and then read from several.
struct RegionRow {
    case: &'static str,
    account: &'static str,
    /// The name the namespace is created under; the model admits len 0..64 over the
    /// pattern `^[a-zA-Z0-9._-]*$`.
    /// Source: smithy_query(query="operation", operation="DescribeNamespace").
    namespace: String,
    /// The region the `CreateNamespace` call is signed for, which is the region whose
    /// free SPICE capacity the namespace uses.
    create_region: &'static str,
    /// The regions the later reads are signed for.
    read_regions: &'static [&'static str],
}

/// The regions below are ordinary commercial AWS regions; nothing in the rule depends
/// on which, only on whether the reader's differs from the create's.
fn region_rows() -> Vec<RegionRow> {
    vec![
        RegionRow {
            case: "read-from-the-creating-region",
            account: ACCOUNT,
            namespace: "sales".to_string(),
            create_region: "us-west-2",
            read_regions: &["us-west-2"],
        },
        RegionRow {
            case: "read-from-two-other-regions",
            account: ACCOUNT,
            namespace: "finance.eu".to_string(),
            create_region: "eu-west-1",
            read_regions: &["us-east-1", "ap-southeast-2", "eu-west-1"],
        },
        RegionRow {
            case: "other-account-same-name",
            account: OTHER_ACCOUNT,
            namespace: "sales".to_string(),
            create_region: "ap-northeast-1",
            read_regions: &["us-west-2", "ap-northeast-1"],
        },
        RegionRow {
            // The longest name the model admits: `Namespace` is len 0..64.
            case: "longest-name",
            account: ACCOUNT,
            namespace: format!("{}.{}", "a".repeat(31), "b".repeat(32)),
            create_region: "us-east-2",
            read_regions: &["eu-central-1"],
        },
    ]
}

#[tokio::test]
async fn describe_reports_the_stored_record_not_the_readers_own_request() {
    let harness = Harness::new();

    for row in region_rows() {
        let case = row.case;
        let namespace = row.namespace.as_str();
        assert!(
            namespace.len() <= 64,
            "case {case:?}: the row's own name is longer than the model admits: \
             {namespace:?}"
        );
        let (status, body) =
            create_namespace(&harness, row.create_region, row.account, namespace).await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with body {body}"
        );

        for read_region in row.read_regions {
            let (status, body) =
                describe_namespace_in(&harness, read_region, row.account, namespace).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read from {read_region} expected 200, got {status} with \
                 body {body}"
            );
            let reported = &body["Namespace"];
            assert_eq!(
                reported["Arn"],
                json!(namespace_arn_in(row.create_region, row.account, namespace)),
                "case {case:?}: read from {read_region} must report the ARN the create \
                 minted: {body}"
            );
            assert_eq!(
                reported["CapacityRegion"],
                json!(row.create_region),
                "case {case:?}: read from {read_region} must report the region the \
                 create ran in: {body}"
            );
            assert_eq!(
                reported["Name"],
                json!(namespace),
                "case {case:?}: read from {read_region} reported the wrong Name: {body}"
            );
            assert_eq!(
                reported["IdentityStore"],
                json!("QUICKSIGHT"),
                "case {case:?}: read from {read_region} reported the wrong \
                 IdentityStore: {body}"
            );
            assert!(
                body["RequestId"]
                    .as_str()
                    .is_some_and(|request_id| !request_id.is_empty()),
                "case {case:?}: read from {read_region} reported no RequestId: {body}"
            );
        }
    }
}

/// One row of the near-miss table: a name that is not the created one, so the account
/// holds no namespace under it.
struct MissRow {
    case: &'static str,
    account: &'static str,
    namespace: &'static str,
}

#[tokio::test]
async fn describe_addresses_one_exact_name_in_one_account() {
    let harness = Harness::new();
    let created = "Sales.eu-west_1";

    let (status, body) = create_namespace(&harness, "us-west-2", ACCOUNT, created).await;
    assert_eq!(
        status, 200,
        "the namespace the near misses are measured against must be created, got \
         {status} with {body}"
    );

    let (status, body) = describe_namespace_in(&harness, "us-west-2", ACCOUNT, created).await;
    assert_eq!(
        status, 200,
        "case \"exact-name\": expected 200, got {status} with body {body}"
    );
    assert_eq!(
        body["Namespace"]["Name"],
        json!(created),
        "case \"exact-name\": the created namespace is the one reported: {body}"
    );

    // Every row below differs from `created` in exactly one way the name's pattern
    // admits — `^[a-zA-Z0-9._-]*$`, len 0..64 — or in the owning account.
    // Source: smithy_query(query="operation", operation="DescribeNamespace").
    let misses = [
        MissRow {
            case: "lowercased-name",
            account: ACCOUNT,
            namespace: "sales.eu-west_1",
        },
        MissRow {
            case: "uppercased-name",
            account: ACCOUNT,
            namespace: "SALES.EU-WEST_1",
        },
        MissRow {
            case: "prefix-of-the-name",
            account: ACCOUNT,
            namespace: "Sales",
        },
        MissRow {
            case: "name-with-a-suffix",
            account: ACCOUNT,
            namespace: "Sales.eu-west_1.2",
        },
        MissRow {
            case: "name-with-the-separator-dropped",
            account: ACCOUNT,
            namespace: "Saleseu-west_1",
        },
        MissRow {
            case: "another-accounts-read-of-the-same-name",
            account: OTHER_ACCOUNT,
            namespace: created,
        },
    ];

    for row in misses {
        let case = row.case;
        let (status, body) =
            describe_namespace_in(&harness, "us-west-2", row.account, row.namespace).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a name no namespace is held under expected 404, got {status} \
             with body {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case:?}: the modelled refusal is ResourceNotFoundException: {body}"
        );
        // The variable parts of the refusal come from the REQUEST — the namespace and
        // the account it addressed — which is what distinguishes one near miss from
        // another. The fixed text around them is asserted once, in
        // `namespace_lifecycle`.
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {body}"));
        assert!(
            message.contains(row.namespace) && message.contains(row.account),
            "case {case:?}: the refusal must name the namespace and the account the \
             request addressed, got {message:?}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("NAMESPACE"),
            "case {case:?}: the refusal is about a namespace: {body}"
        );
    }
}

#[tokio::test]
async fn describing_a_namespace_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    let namespace = "reporting";

    let (status, body) = create_namespace(&harness, "us-west-2", ACCOUNT, namespace).await;
    assert_eq!(
        status, 200,
        "create expected 200, got {status} with body {body}"
    );
    // With the transition the clock owes already due, the record's report is settled,
    // so repeated reads differing would be the read itself changing something.
    settle_creation(&harness, ACCOUNT, namespace);

    let before = stored_names(&harness, ACCOUNT);
    let mut reports = Vec::new();
    for read in 0..3 {
        let (status, body) = describe_namespace_in(&harness, "us-west-2", ACCOUNT, namespace).await;
        assert_eq!(
            status, 200,
            "case \"read-{read}\": expected 200, got {status} with body {body}"
        );
        reports.push(body["Namespace"].clone());
    }
    for (read, report) in reports.iter().enumerate() {
        assert_eq!(
            report, &reports[0],
            "case \"read-{read}\": every read of one namespace reports the same record, \
             got {report}"
        );
        assert_eq!(
            report["CreationStatus"],
            json!("CREATED"),
            "case \"read-{read}\": a settled namespace stays ready to use, got {report}"
        );
    }
    assert_eq!(
        stored_names(&harness, ACCOUNT),
        before,
        "reading a namespace must not change what the account holds"
    );

    // A read that finds nothing stores nothing either, so a later caller still finds
    // nothing under that name.
    for case in ["never-created", "also-never-created"] {
        let (status, body) = describe_namespace_in(&harness, "us-west-2", ACCOUNT, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with body {body}"
        );
        assert_eq!(
            stored_names(&harness, ACCOUNT),
            before,
            "case {case:?}: a refused read must not mint the namespace it looked for"
        );
    }
}

#[tokio::test]
async fn the_default_namespace_is_reported_from_the_account_and_mints_nothing() {
    let harness = Harness::new();

    // The namespace the model's `Namespace` members tell a caller with no custom
    // namespace to address is held by every account, and no create minted it — so it
    // is reported in whichever region the account is reached in, and reading it leaves
    // the account holding no namespace record at all.
    // Source: smithy_query(query="doc", shape="DescribeNamespaceRequest",
    // member="Namespace").
    for (case, account, region) in [
        ("untouched-account", ACCOUNT, "us-west-2"),
        ("same-account-other-region", ACCOUNT, "eu-west-1"),
        ("another-account", OTHER_ACCOUNT, "ap-southeast-2"),
    ] {
        let mut reports = Vec::new();
        for read in 0..2 {
            let (status, body) = describe_namespace_in(&harness, region, account, "default").await;
            assert_eq!(
                status, 200,
                "case {case:?} read {read}: the default namespace is describable, got \
                 {status} with body {body}"
            );
            reports.push(body["Namespace"].clone());
        }
        let reported = &reports[0];
        assert_eq!(
            reported, &reports[1],
            "case {case:?}: two reads of the default namespace must report the same \
             record, got {reported} then {}",
            reports[1]
        );
        assert_eq!(
            reported["Name"],
            json!("default"),
            "case {case:?}: reported the wrong Name: {reported}"
        );
        assert_eq!(
            reported["Arn"],
            json!(namespace_arn_in(region, account, "default")),
            "case {case:?}: the default namespace's ARN carries its account and region: \
             {reported}"
        );
        assert_eq!(
            reported["CapacityRegion"],
            json!(region),
            "case {case:?}: the default namespace is reported in the region the account \
             is reached in: {reported}"
        );
        assert_eq!(
            reported["CreationStatus"],
            json!("CREATED"),
            "case {case:?}: the default namespace is ready to use: {reported}"
        );
        assert!(
            stored_names(&harness, account).is_empty(),
            "case {case:?}: describing the default namespace must mint no record, the \
             account holds {:?}",
            stored_names(&harness, account)
        );
    }
}
