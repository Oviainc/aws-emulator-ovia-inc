// SPDX-License-Identifier: CC0-1.0

//! A dataset's type is reported in ONE spelling, whatever casing the create wrote.
//!
//! `CreateDataset` accepts `datasetType` "case insensitive" (the model's own wording for
//! the member), and Amazon Personalize identifies the type by its UPPER-CASE name — the
//! spelling the dataset ARN's trailing segment carries. So the type a read reports is that
//! upper-case name, not the characters the request happened to use, and two requests that
//! name one type in different casings describe the same dataset identically.
//!
//! The rows below differ along what that rule generalises over: which documented type is
//! named, and how the request spelled it (as documented, upper-cased, lower-cased,
//! alternating case, and a partly-cased underscored name). Successive creates of ONE
//! dataset in different casings are covered in `dataset_type_echo.rs`.

use crate::dataset_group_lifecycle::{
    await_active, avro_schema, create_active_group, create_schema, error_code, Emulator, ACCOUNT,
    REGION,
};

use serde_json::json;

/// One row: a documented dataset type, and the spelling a request writes it in.
struct CasingCase {
    /// The type as the model's documentation lists it. Source: the documentation for
    /// `CreateDatasetRequest$datasetType` — "One of the following (case insensitive)
    /// values: Interactions, Items, Users, Actions, Action_Interactions".
    documented: &'static str,
    /// What the request sends, character for character.
    requested: &'static str,
}

/// Every documented type, each in a spelling a caller may write. The upper-cased
/// `Interactions` row is the one the trace recorded; the others are types and casings it
/// did not name, including both spellings of one underscored type.
const CASING_CASES: &[CasingCase] = &[
    CasingCase {
        documented: "Interactions",
        requested: "INTERACTIONS",
    },
    CasingCase {
        documented: "Items",
        requested: "items",
    },
    CasingCase {
        documented: "Users",
        requested: "Users",
    },
    CasingCase {
        documented: "Actions",
        requested: "aCtIoNs",
    },
    CasingCase {
        documented: "Action_Interactions",
        requested: "ACTION_INTERACTIONS",
    },
    CasingCase {
        documented: "Action_Interactions",
        requested: "action_Interactions",
    },
];

/// A different spelling of `documented` from `requested`, so a second create names the
/// SAME type in other characters.
fn other_spelling(documented: &str, requested: &str) -> String {
    let lowered = documented.to_ascii_lowercase();
    if lowered == requested {
        documented.to_ascii_uppercase()
    } else {
        lowered
    }
}

#[tokio::test]
async fn describe_and_list_report_the_dataset_type_in_its_upper_case_name() {
    let emulator = Emulator::new();

    for (index, row) in CASING_CASES.iter().enumerate() {
        let case = format!("datasetType {:?} (type {})", row.requested, row.documented);
        let reported = row.documented.to_ascii_uppercase();

        // One group per row, so each row's dataset is the only one of its type in its
        // group.
        let group_name = format!("casing-dsg-{index}");
        let group_arn = create_active_group(&emulator, &group_name, &case).await;
        let schema_arn = create_schema(
            &emulator,
            &format!("casing-schema-{index}"),
            &avro_schema(row.documented),
            &case,
        )
        .await;

        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": format!("casing-ds-{index}"),
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": row.requested,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataset to answer 200, got {status} {body:?}"
        );
        let arn = body["datasetArn"].as_str().unwrap_or_default().to_string();

        // The ARN's trailing segment is the same upper-case name, whatever the request's
        // casing, so the reported type and the ARN agree.
        assert_eq!(
            arn,
            format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/{group_name}/{reported}"
            ),
            "case {case:?}: expected the minted dataset ARN, got {arn:?}"
        );

        let dataset = await_active(
            &emulator,
            "DescribeDataset",
            "datasetArn",
            "dataset",
            &arn,
            &case,
        )
        .await;
        assert_eq!(
            dataset["datasetType"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: expected DescribeDataset to report {reported:?}, got {:?}",
            dataset["datasetType"]
        );
        assert!(
            arn.ends_with(dataset["datasetType"].as_str().unwrap_or_default()),
            "case {case:?}: expected the reported type to be the ARN's type segment, got \
             {dataset:?} for {arn:?}"
        );

        let (status, body) = emulator
            .call("ListDatasets", json!({"datasetGroupArn": group_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListDatasets to answer 200, got {status} {body:?}"
        );
        let summaries = body["datasets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a datasets list, got {body:?}"))
            .clone();
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected the group's single dataset, got {summaries:?}"
        );
        assert_eq!(
            summaries[0]["datasetType"].as_str(),
            Some(reported.as_str()),
            "case {case:?}: expected ListDatasets to report {reported:?}, got {:?}",
            summaries[0]["datasetType"]
        );

        // Case-insensitive acceptance: another spelling of the same type is the same
        // type, so the group still holds only one of it.
        let second = other_spelling(row.documented, row.requested);
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": format!("casing-ds-{index}-again"),
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": second,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second dataset spelled {second:?} to be refused, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceAlreadyExistsException"),
            "case {case:?}: expected ResourceAlreadyExistsException for {second:?}, got {body:?}"
        );
    }
}
