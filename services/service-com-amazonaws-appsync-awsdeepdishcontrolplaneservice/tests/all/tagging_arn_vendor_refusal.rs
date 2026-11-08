// SPDX-License-Identifier: CC0-1.0

//! How the tagging surface refuses a `resourceArn` whose VENDOR field names another
//! service, end to end over HTTP.
//!
//! One rule, asserted over a TABLE spanning the class it generalises over: the surface
//! reads the ARN's vendor field — the third of
//! `arn:<partition>:<vendor>:<region>:<account>:<resource>` — before it reads anything
//! else about the value, and an ARN of a vendor this service does not serve is refused
//! with the sentence that names the vendor it expected (400 `BadRequestException`,
//! "Invalid input resource arn: expected vendor appsync"), whichever modelled constraint
//! the value also happens to break. Three groups of rows span it:
//!
//! 1. ARNs of OTHER vendors, varying along what the rule generalises over — the vendor
//!    token, the ARN's length relative to the member's modelled 70..75 window, the
//!    partition, whether the Region and account fields are filled or empty (AWS writes a
//!    partition-global or account-less resource with them empty), and whether the
//!    resource is written `<type>/<id>` or `<type>:<id>` — each of which takes the
//!    expected-vendor sentence;
//! 2. CONTROL rows whose vendor field IS this service's own, broken in some other field
//!    instead (length, partition, account, resource type, id alphabet) plus text that is
//!    no ARN at all: the vendor clause is about the vendor field, so those keep the bare
//!    malformed-ARN sentence;
//! 3. the happy path of the same check: a resource created with tags still round-trips
//!    through `ListTagsForResource`, so the vendor reading refuses nothing it serves.
//!
//! The rule belongs to the one `/v1/tags/{resourceArn}` route rather than to one
//! operation, so every row is asserted on all three operations that route carries. A
//! member of ANOTHER shape that merely shares the name `resourceArn`
//! (`RdsDataApiConfig$resourceArn`, an RDS cluster ARN) is not this surface's member, and
//! the last case here pins that its refusal claims no appsync vendor.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, the `@httpLabel` ARN binding, the boundary validator, restJson1
//! (de)serialization and this service's refusal wording together. The harness is the
//! error-contract suite's `call`/`Answer` over the lifecycle suite's router and create
//! helper, with the tag suites' request helpers.

use serde_json::json;

use crate::api_key_version_and_tagging_refusals::tagging_calls;
use crate::error_contract::call;
use crate::graphql_api_lifecycle::{create_api, router, REGION};
use crate::resource_tags::{ABSENT_API_ID, ACCOUNT};
use crate::tag_resource::list_tags;

/// The sentence the tagging surface refuses an ARN of another vendor with: the fixed text
/// followed by the vendor it expected, which for this service is its own ARN namespace
/// (`appsync`, the namespace `ResourceArn`'s modelled pattern
/// `^arn:aws:appsync:…:apis/…$` fixes — `smithy_query(query="operation",
/// operation="ListTagsForResource")`). Nothing of the submitted ARN is echoed.
pub(crate) const EXPECTED_VENDOR_REFUSAL: &str =
    "Invalid input resource arn: expected vendor appsync";

/// The sentence a refusal keeps when the value's vendor field is not what failed.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn";

/// The status and error shape every one of these refusals takes: the one client error the
/// tagging operations model (`smithy_query(query="operation",
/// operation="ListTagsForResource")` — `BadRequestException`, `httpError` 400).
const BAD_REQUEST: (u16, &str) = (400, "BadRequestException");

/// ARN namespaces of OTHER services, for the ARNs the refusal has to diagnose by vendor.
///
/// Source: this model's own other ARN members, whose patterns name the vendors AppSync
/// resolves ARNs of — `rds` and `secretsmanager` (`RdsDataApiConfig$resourceArn`
/// `^arn:[a-z-]*:rds:…:cluster:…$` and `RdsDataApiConfig$secretArn`
/// `^arn:[a-z-]*:secretsmanager:…:secret:…$`, `smithy_query(query="shape",
/// shape="RdsDataApiConfig")`) and `acm`/`iam` (`CertificateArn`'s
/// `^arn:[a-z-]*:(acm|iam):…$`, `smithy_query(query="shape", shape="CertificateArn")`).
/// `s3` is the one AWS writes with empty Region and account fields
/// (`arn:aws:s3:::<bucket>`), the form this tree's ARN reading documents as well formed.
const OTHER_VENDORS: &[&str] = &["acm", "iam", "rds", "s3", "secretsmanager"];

/// One row: what the value is, and the sentence the rule says it is refused with.
#[derive(Debug)]
struct Case {
    label: String,
    candidate: String,
    expected: &'static str,
}

impl Case {
    /// A row the vendor reading must diagnose by name.
    fn foreign(label: &str, candidate: String) -> Self {
        Self {
            label: label.to_string(),
            candidate,
            expected: EXPECTED_VENDOR_REFUSAL,
        }
    }

    /// A row whose vendor field is this service's own (or absent), so the bare sentence
    /// stands.
    fn own_vendor(label: &str, candidate: String) -> Self {
        Self {
            label: label.to_string(),
            candidate,
            expected: INVALID_RESOURCE_ARN,
        }
    }
}

/// The table: the vendor rows, then the control rows.
fn cases() -> Vec<Case> {
    let mut rows: Vec<Case> = OTHER_VENDORS
        .iter()
        .map(|vendor| {
            Case::foreign(
                &format!("an ARN of vendor {vendor} in this account and Region"),
                format!("arn:aws:{vendor}:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
            )
        })
        .collect();

    rows.extend([
        // The recorded shape of the defect: an ARN of another vendor with empty Region
        // and account fields, longer than the member's modelled maximum.
        Case::foreign(
            "an account-less ARN of another vendor over the modelled maximum length",
            "arn:aws:s3:::not-an-appsync-resource-padded-to-clear-the-minimum-arn-length-check"
                .to_string(),
        ),
        Case::foreign(
            "an ARN of another vendor under the modelled minimum length",
            "arn:aws:s3:::bucket".to_string(),
        ),
        Case::foreign(
            "an ARN of another vendor writing its resource as type:id",
            format!("arn:aws:states:{REGION}:{ACCOUNT}:execution:machine:run"),
        ),
        Case::foreign(
            "an ARN of another vendor in another partition",
            format!("arn:aws-us-gov:lambda:{REGION}:{ACCOUNT}:function:worker"),
        ),
        Case::foreign(
            "an ARN of another vendor with an empty Region",
            format!("arn:aws:iam::{ACCOUNT}:role/service-role"),
        ),
        Case::foreign(
            "this vendor's namespace in the wrong case",
            format!("arn:aws:AppSync:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
        Case::foreign(
            "this vendor's namespace with a trailing character",
            format!("arn:aws:appsyncx:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
        // Controls: the vendor field is this service's own, or there is none to read.
        Case::own_vendor(
            "this vendor in another partition",
            format!("arn:xws:appsync:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
        Case::own_vendor(
            "this vendor with an account that is not twelve digits",
            format!("arn:aws:appsync:{REGION}:00000000000a:apis/{ABSENT_API_ID}"),
        ),
        Case::own_vendor(
            "this vendor with another resource type",
            format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apiz/{ABSENT_API_ID}"),
        ),
        Case::own_vendor(
            "this vendor with an id outside the id alphabet",
            format!(
                "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}!",
                &ABSENT_API_ID[..25]
            ),
        ),
        Case::own_vendor(
            "this vendor under the modelled minimum length",
            format!(
                "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}",
                &ABSENT_API_ID[..25]
            ),
        ),
        Case::own_vendor("no ARN at all", "not-an-arn".to_string()),
        Case::own_vendor(
            "an ARN prefix with no resource field",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}"),
        ),
        Case::own_vendor(
            "an ARN with an empty vendor field",
            format!("arn:aws::{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
        ),
    ]);
    rows
}

#[tokio::test]
async fn tagging_arn_refusal_names_the_vendor_it_expected() {
    let router = router();

    for case in cases() {
        for (operation, answer) in tagging_calls(&router, &case.candidate).await {
            assert_eq!(
                (
                    answer.status,
                    answer.error_type.as_str(),
                    answer.message.as_str()
                ),
                (BAD_REQUEST.0, BAD_REQUEST.1, case.expected),
                "case {:?} ({}) on {operation}: expected {} {} {:?}, got {answer:?}",
                case.label,
                case.candidate,
                BAD_REQUEST.0,
                BAD_REQUEST.1,
                case.expected
            );
        }
    }
}

#[tokio::test]
async fn the_vendor_reading_refuses_nothing_the_surface_serves() {
    let router = router();
    let created = create_api(
        &router,
        json!({
            "name": "tagged",
            "authenticationType": "API_KEY",
            "tags": { "Team": "checkout", "Env": "test" },
        }),
    )
    .await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case the created API: expected an ARN, got {created:?}"))
        .to_string();

    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        (listed.status, &listed.body["tags"]),
        (200, &json!({ "Team": "checkout", "Env": "test" })),
        "case the created API ({arn}): expected 200 with the tags it was created with, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn another_shapes_resource_arn_claims_no_appsync_vendor() {
    let router = router();
    // `RdsDataApiConfig$resourceArn` is "the resource ARN of the RDS cluster", a member of
    // another vendor's ARNs that merely shares the tagging member's name; a value of a
    // third vendor is refused there, but the vendor the TAGGING surface expects is not the
    // vendor that member names.
    let answer = call(
        &router,
        "POST",
        "/v1/datasources/introspections",
        Some(json!({
            "rdsDataApiConfig": {
                "resourceArn": "arn:aws:s3:::not-an-rds-cluster-or-an-appsync-api",
                "secretArn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:db-creds"),
                "databaseName": "inventory",
            }
        })),
    )
    .await;
    assert!(
        !answer.message.contains(EXPECTED_VENDOR_REFUSAL),
        "case a nested resourceArn of another shape: expected a refusal claiming no \
         {EXPECTED_VENDOR_REFUSAL:?}, got {answer:?}"
    );
}
