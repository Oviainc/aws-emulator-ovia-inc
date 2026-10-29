// SPDX-License-Identifier: CC0-1.0

//! The two places this service reads a FIELD of an ARN, over the awsJson1_1 wire.
//!
//! The rule under test: an ARN is the six colon-separated fields
//! `arn:<partition>:<vendor>:<region>:<account>:<resource>`, whose FIFTH field is the
//! account and whose SIXTH field is the whole resource — every colon it carries
//! included — and both reads this service makes take their field by POSITION in that one
//! grammar. The two reads are:
//!
//! 1. the tagging operations' ACCOUNT check, which refuses a `ResourceArn` naming another
//!    account with the modelled `ValidationException` before any lookup, and looks an ARN
//!    of this account up (`crate::tagging_arn_validation` pins that refusal's wording and
//!    ordering; this suite pins which field it is decided from);
//! 2. the authorization layer's PRINCIPAL resolution, which reads the role of an STS
//!    `assumed-role/<role>/<session>` session and the name of an IAM `user/<path…>/<name>`
//!    out of the resource field, to find the identity-based documents in force on the
//!    caller (`crate::front_door_authorization` and `crate::untagging_authorization` pin
//!    the decision those documents produce; this suite pins that the identity is found at
//!    all).
//!
//! Both now read that grammar from the one parse the tree shares, so neither can disagree
//! with the other — or with the services that MINT these ARNs — about which field is which.
//! The consequences are observable, and each row below separates the position from a value
//! that would also match a search: an own-account ARN whose resource segment carries a
//! twelve-digit group of ANOTHER account is still this account's (so it is looked up), a
//! foreign ARN whose resource segment carries THIS account's id is still foreign (so it is
//! refused), a resource field carrying colons stays one field, and a principal ARN with an
//! EMPTY region field — which is how STS and IAM mint theirs — still resolves to its
//! identity.
//!
//! Every `ResourceArn` below satisfies the modelled `Arn` constraints (`@length 20..2048`,
//! `@pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`
//! — smithy_query shape `Arn`), so every answer comes from the handler rather than from
//! boundary validation.

use serde_json::json;

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::RESOURCE_NOT_TAGGABLE_MESSAGE;

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};
use crate::front_door_authorization::{allow, export_arn_pattern, Emulator, ACCOUNT_KEY};

/// The export the authorization table names.
const EXPORT_NAME: &str = "shared_arn_parse_export";

/// A foreign account id, used where the row is about the account FIELD rather than about
/// which account it is.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// What the tagging read must answer for one ARN.
#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// An ARN of this account naming the export that exists: its tags.
    Served,
    /// An ARN of this account naming nothing: looked up, and not found.
    LookedUp,
    /// An ARN of another account: refused on the ARN, with no lookup.
    Refused,
}

/// An export ARN naming `account`, in `partition` and `region`, for `resource`.
fn arn(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:bcm-data-exports:{region}:{account}:{resource}")
}

/// The account every ARN this emulator mints names.
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The resource field of `value`: everything after the fifth colon, read here by hand so
/// the table's expectations do not come from the code under test.
fn resource_field(value: &str) -> String {
    value
        .splitn(6, ':')
        .nth(5)
        .expect("a minted ARN carries a resource field")
        .to_string()
}

#[tokio::test]
async fn the_tagging_account_check_reads_the_account_field() {
    let client = Client::new();

    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let own_arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    let own_resource = resource_field(&own_arn);

    let rows: Vec<(String, String, Expected)> = vec![
        (
            "the export this account holds".to_string(),
            own_arn.clone(),
            Expected::Served,
        ),
        // This account, with a resource field that carries ANOTHER account's twelve-digit
        // id: the account is the fifth field, not whichever group of digits looks like one.
        (
            "this account, naming another account id in its resource".to_string(),
            arn(
                "aws",
                "us-east-1",
                own_account(),
                &format!("export/{FOREIGN_ACCOUNT}/probe"),
            ),
            Expected::LookedUp,
        ),
        // This account, with a resource field of its own carrying COLONS: `…:type:id` is
        // one field, so the fields before it keep their positions.
        (
            "this account, with colons inside the resource".to_string(),
            arn(
                "aws",
                "us-east-1",
                own_account(),
                "export:absent:00000000-0000-0000-0000-000000000000",
            ),
            Expected::LookedUp,
        ),
        // This account with an EMPTY region field: still five fields before the resource.
        (
            "this account with an empty region".to_string(),
            arn("aws", "", own_account(), "export/absent"),
            Expected::LookedUp,
        ),
        // This account in another partition: the partition is not the account.
        (
            "this account in the gov partition".to_string(),
            arn("aws-us-gov", "us-gov-west-1", own_account(), "export/absent"),
            Expected::LookedUp,
        ),
        // Another account whose resource field carries THIS account's id: the ARN is still
        // foreign, so it is refused without a lookup.
        (
            "another account naming this account id in its resource".to_string(),
            arn(
                "aws",
                "us-east-1",
                FOREIGN_ACCOUNT,
                &format!("export/{}/probe", own_account()),
            ),
            Expected::Refused,
        ),
        // Another account naming, byte for byte, the resource of the export this account
        // holds.
        (
            "another account naming this account's own export".to_string(),
            arn("aws", "us-east-1", FOREIGN_ACCOUNT, &own_resource),
            Expected::Refused,
        ),
        (
            "another account with an empty region".to_string(),
            arn("aws", "", "999999999999", "export/absent"),
            Expected::Refused,
        ),
        (
            "another account with colons inside the resource".to_string(),
            arn(
                "aws-cn",
                "cn-north-1",
                "777788889999",
                "export:absent:0000_1111",
            ),
            Expected::Refused,
        ),
    ];

    for (label, candidate, expected) in &rows {
        let (status, body) = client
            .call(
                "ListTagsForResource",
                &json!({ "ResourceArn": candidate }),
            )
            .await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: an export of this account must have its tags reported, \
                     got {status} with {body}"
                );
                assert!(
                    body["ResourceTags"].is_array(),
                    "case {label:?}: expected a ResourceTags list, got {body}"
                );
            }
            Expected::LookedUp => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                    "case {label:?}: an ARN of this account must be looked up, got {status} \
                     with {body}"
                );
                assert_eq!(
                    body["ResourceId"].as_str(),
                    Some(candidate.as_str()),
                    "case {label:?}: the not-found must report the ARN it looked for, got {body}"
                );
            }
            Expected::Refused => {
                assert_eq!(
                    (status, error_code(&body).as_str(), body["Message"].as_str()),
                    (
                        400,
                        "ValidationException",
                        Some(RESOURCE_NOT_TAGGABLE_MESSAGE)
                    ),
                    "case {label:?}: an ARN of another account must be refused on the ARN, \
                     got {status} with {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_principal_resolution_reads_the_resource_field() {
    let emulator = Emulator::new();

    let (status, created) = emulator
        .call(ACCOUNT_KEY, "CreateExport", &{
            let mut request = cases().remove(0).create_request();
            request["Export"]
                .as_object_mut()
                .expect("Export is a JSON object")
                .insert("Name".to_string(), json!(EXPORT_NAME));
            request
        })
        .await;
    assert_eq!(status, 200, "the setup create must succeed, got {created}");
    let export_arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    let grant = allow(
        "bcm-data-exports:ListTagsForResource",
        &export_arn_pattern(EXPORT_NAME),
    );

    // One row per form of principal ARN the resolution has to read: an STS session
    // (`assumed-role/<role>/<session>`, minted with an EMPTY region field), an IAM user
    // (`user/<name>`, minted the same way), and an id neither store holds. A principal
    // whose grant is found can only have been found by reading its ARN's resource field;
    // one whose documents are not read is refused whatever its policy says.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "an assumed-role session granted this action on this export",
            emulator
                .assumed_role_key("bcmde-shared-arn-granted", Some(&grant))
                .await,
            true,
        ),
        (
            "an assumed-role session whose role name carries _ - and .",
            emulator
                .assumed_role_key(
                    "bcmde_shared-arn.wildcard",
                    Some(&allow("bcm-data-exports:*", "*")),
                )
                .await,
            true,
        ),
        (
            "an assumed-role session with no policy",
            emulator
                .assumed_role_key("bcmde-shared-arn-bare", None)
                .await,
            false,
        ),
        (
            "an IAM user granted this action on this export",
            emulator
                .user_key("bcmde-shared-arn-user-granted", Some(&grant))
                .await,
            true,
        ),
        (
            "an IAM user granted only the sibling write",
            emulator
                .user_key(
                    "bcmde-shared-arn-user-sibling",
                    Some(&allow("bcm-data-exports:TagResource", "*")),
                )
                .await,
            false,
        ),
        (
            "an IAM user with no policy",
            emulator.user_key("bcmde-shared-arn-user-bare", None).await,
            false,
        ),
        (
            "an access key id neither store holds",
            "AKIDNOBODYHOLDS".to_string(),
            true,
        ),
    ];

    for (label, access_key_id, allowed) in &rows {
        let (status, body) = emulator
            .call(
                access_key_id,
                "ListTagsForResource",
                &json!({ "ResourceArn": &export_arn }),
            )
            .await;
        if *allowed {
            assert_eq!(
                status, 200,
                "case {label:?}: a principal whose documents allow the action must be served, \
                 got {status} with {body}"
            );
            assert!(
                body["ResourceTags"].is_array(),
                "case {label:?}: expected a ResourceTags list, got {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: a principal whose documents do not allow the action must be \
                 refused, got {status} with {body}"
            );
        }
    }

    // Both reads in ONE call: the caller is resolved from its session ARN's resource field
    // and allowed, and the `ResourceArn` it names is then judged on its account field.
    let granted = emulator
        .assumed_role_key(
            "bcmde-shared-arn-both",
            Some(&allow("bcm-data-exports:*", "*")),
        )
        .await;
    let foreign = arn(
        "aws",
        "us-east-1",
        FOREIGN_ACCOUNT,
        &resource_field(&export_arn),
    );
    let (status, body) = emulator
        .call(
            &granted,
            "ListTagsForResource",
            &json!({ "ResourceArn": foreign }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (
            400,
            "ValidationException",
            Some(RESOURCE_NOT_TAGGABLE_MESSAGE)
        ),
        "case \"an allowed session naming another account's ARN\": the account field is \
         judged after the caller is resolved, got {status} with {body}"
    );
}
