// SPDX-License-Identifier: CC0-1.0

//! The `ResourceArn` a tagging operation accepts, over the awsJson1_1 wire.
//!
//! The rule under test: a tagging operation validates its `ResourceArn`'s ACCOUNT BEFORE
//! any existence lookup, so an ARN naming an account other than the caller's own is
//! refused with the modelled `ValidationException` (HTTP 400) and never looked up — while
//! an ARN of the caller's own account is looked up, and answers either the resource's tags
//! or the modelled `ResourceNotFoundException`. The two codes mean different things
//! to a client: not-found invites a create-then-retry, a validation failure says the ARN
//! itself is unusable. Deciding a foreign ARN without a lookup is also what keeps the
//! answer from depending on what this account holds, which is the cross-tenant probe the
//! ordering rules out. It holds for every tagging operation this service serves —
//! `ListTagsForResource` and `TagResource` below; `UntagResource` is still a 501 stub under
//! its own issue and will call the same check.
//!
//! It is asserted over a TABLE of ARNs that all satisfy the modelled `Arn` constraints
//! (`@length 20..2048`, `@pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`
//! — smithy_query shape `Arn`), so every answer below comes from the handler and not from
//! boundary validation. The rows differ along what the rule generalises over: the account
//! (the caller's own and several foreign ones), the partition, the region, and the
//! resource segment — including a foreign ARN whose resource segment is byte-for-byte the
//! one of an export this account really holds.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::RESOURCE_NOT_TAGGABLE_MESSAGE;

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// The account every ARN this emulator mints names (`engine::emulator_account_id`).
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// An export ARN naming `account`, in `partition` and `region`, for `resource`.
fn arn(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:bcm-data-exports:{region}:{account}:{resource}")
}

/// Foreign-account ARNs: the accounts, partitions, regions and resource forms the rule
/// has to hold for. `resource` is varied too, so nothing in the rule can key on it.
fn foreign_arns(own_resource: &str) -> Vec<(String, String)> {
    let mut rows = vec![
        // The recorded case.
        (
            "the recorded cross-account ARN".to_string(),
            arn(
                "aws",
                "us-east-1",
                "111122223333",
                "export/isolated-test/00000000-0000-0000-0000-000000000000",
            ),
        ),
        (
            "a foreign account in another region".to_string(),
            arn("aws", "ap-southeast-2", "210987654321", "export/other_tenant"),
        ),
        (
            "a foreign account in the gov partition".to_string(),
            arn("aws-us-gov", "us-gov-west-1", "444455556666", "export/absent"),
        ),
        (
            "a foreign account in the china partition".to_string(),
            arn("aws-cn", "cn-north-1", "777788889999", "export/absent"),
        ),
        (
            "a foreign account with an empty region".to_string(),
            arn("aws", "", "123456789012", "export/absent"),
        ),
        // The cross-tenant probe: the resource segment of an export this account DOES
        // hold, under somebody else's account.
        (
            "another account naming an export this account holds".to_string(),
            arn("aws", "us-east-1", "999999999999", own_resource),
        ),
    ];
    // One account off by a single digit from this emulator's own, so the comparison is on
    // the whole account and not a prefix of it.
    let mut neighbour: Vec<char> = own_account().chars().collect();
    let last = neighbour.len() - 1;
    neighbour[last] = if neighbour[last] == '9' { '8' } else { '9' };
    rows.push((
        "an account one digit from this one".to_string(),
        arn(
            "aws",
            "us-east-1",
            &neighbour.iter().collect::<String>(),
            own_resource,
        ),
    ));
    rows
}

/// The resource segment of the ARN this service mints for the export called `name`.
fn own_resource_of(arn: &str) -> String {
    arn.splitn(6, ':')
        .nth(5)
        .expect("a minted ARN carries a resource segment")
        .to_string()
}

#[tokio::test]
async fn a_tagging_arn_of_another_account_is_refused_before_any_lookup() {
    let client = Client::new();

    // An export of this account, so the table can name its resource segment under a
    // foreign account and the refusal can be compared against a real lookup.
    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let own_arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    let own_resource = own_resource_of(&own_arn);

    let mut refusals: Vec<Value> = Vec::new();
    for (label, foreign) in foreign_arns(&own_resource) {
        // Both tagging operations this service serves read the ARN the same way. (The
        // third, `UntagResource`, is still a 501 stub under its own issue and will call
        // the same check.)
        for (operation, request) in [
            (
                "ListTagsForResource",
                json!({ "ResourceArn": foreign }),
            ),
            (
                "TagResource",
                json!({
                    "ResourceArn": foreign,
                    "ResourceTags": [{ "Key": "env", "Value": "dev" }],
                }),
            ),
        ] {
            let label = format!("{label} / {operation}");
            let (status, body) = client.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?}: expected HTTP 400 ValidationException for {foreign}, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(RESOURCE_NOT_TAGGABLE_MESSAGE),
                "case {label:?}: expected the fixed tagging refusal, got {body}"
            );
            // The refusal reports nothing about the ARN: it was never looked up.
            assert!(
                !body["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&foreign),
                "case {label:?}: the refusal must not restate the ARN, got {body}"
            );
            if operation == "ListTagsForResource" {
                refusals.push(body);
            }
        }
    }
    let first = refusals
        .first()
        .expect("the table has rows")
        .clone();
    for (index, body) in refusals.iter().enumerate() {
        assert_eq!(
            *body, first,
            "case {index}: every foreign ARN must get the SAME answer, so it cannot report \
             on what this account holds"
        );
    }

    // The ARN check comes FIRST: a foreign ARN carrying a pagination token this service
    // never issued is still refused on the ARN, not on the token.
    let (status, body) = client
        .call(
            "ListTagsForResource",
            &json!({
                "ResourceArn": arn("aws", "us-east-1", "111122223333", "export/absent"),
                "NextToken": "not-a-token-this-service-issued",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (400, "ValidationException", Some(RESOURCE_NOT_TAGGABLE_MESSAGE)),
        "a foreign ARN must be refused on the ARN before anything else is read, got {body}"
    );
}

#[tokio::test]
async fn a_tagging_arn_of_this_account_is_looked_up() {
    let client = Client::new();

    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let own_arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // An ARN of this account that names an existing export: served.
    let tags = client
        .ok("ListTagsForResource", &json!({ "ResourceArn": own_arn }))
        .await;
    assert!(
        tags["ResourceTags"].is_array(),
        "an export of this account must have its tags reported, got {tags}"
    );

    // ARNs of this account that name nothing: looked up, and not found — including the
    // multi-segment and short resource forms the tagging check does not judge.
    let absent = [
        ("never created", own_resource_of(&own_arn) + "_absent"),
        ("a deeper resource path", "export/absent/00000000".to_string()),
        ("a single-character resource", "a".to_string()),
        ("another resource class", "table/COST_AND_USAGE_REPORT".to_string()),
    ];
    for (label, resource) in absent {
        let candidate = arn("aws", "us-east-1", own_account(), &resource);
        let (status, body) = client
            .call("ListTagsForResource", &json!({ "ResourceArn": candidate }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {label:?}: an ARN of this account is looked up, so it must answer the \
             not-found refusal, got {status} with {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(candidate.as_str()),
            "case {label:?}: the refusal must report the ARN it could not find, got {body}"
        );
    }

    // GetExport is unchanged by this rule: it answers a foreign ARN with the not-found
    // refusal its own lookup reaches, which is what the tagging path must NOT do.
    let foreign = arn(
        "aws",
        "us-east-1",
        "111122223333",
        "export/isolated-test/00000000-0000-0000-0000-000000000000",
    );
    let (status, body) = client
        .call("GetExport", &json!({ "ExportArn": foreign }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "GetExport on a foreign ARN stays a not-found refusal, got {status} with {body}"
    );
}
