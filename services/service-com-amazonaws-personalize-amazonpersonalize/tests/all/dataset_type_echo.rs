// SPDX-License-Identifier: CC0-1.0

//! Whether a read ECHOES the characters a create wrote for `datasetType`: it does not.
//!
//! `CreateDataset` accepts the member "case insensitive" (the model's own wording), and
//! Amazon Personalize identifies the type by its UPPER-CASE name — the spelling the
//! dataset ARN's trailing segment carries. So one type reads back the same however the
//! request that created the dataset spelled it, and the value a client reads is the type's
//! name rather than its own input.
//!
//! The table of types and casings lives in `dataset_type_casing.rs`; this suite varies the
//! casing of ONE type across successive creates of the same dataset.

use crate::dataset_group_lifecycle::{
    await_active, avro_schema, create_active_group, create_schema, Emulator, ACCOUNT, REGION,
};

use serde_json::json;

/// Four spellings of one documented type: as documented, upper-cased, lower-cased and
/// alternating. Source: the documentation for `CreateDatasetRequest$datasetType` — "One of
/// the following (case insensitive) values: Interactions, Items, Users, Actions,
/// Action_Interactions"; the upper-cased spelling is the one the trace recorded.
const CASINGS_OF_ONE_TYPE: &[&str] = &[
    "INTERACTIONS",
    "interactions",
    "Interactions",
    "iNtErAcTiOnS",
];

#[tokio::test]
async fn one_type_reads_the_same_whichever_casing_created_the_dataset() {
    let emulator = Emulator::new();

    let setup = "recreate in another casing";
    let group_arn = create_active_group(&emulator, "casing-recreate-dsg", setup).await;
    let schema_arn = create_schema(
        &emulator,
        "casing-recreate-schema",
        &avro_schema("Interactions"),
        setup,
    )
    .await;

    // Deleting and recreating the same type under a different casing reports the same
    // type, and the same ARN: the reported value belongs to the type, not to the request.
    for requested in CASINGS_OF_ONE_TYPE {
        let case = format!("{setup}: datasetType {requested:?}");
        let (status, body) = emulator
            .call(
                "CreateDataset",
                json!({
                    "name": "casing-recreate-ds",
                    "schemaArn": schema_arn,
                    "datasetGroupArn": group_arn,
                    "datasetType": requested,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDataset to answer 200, got {status} {body:?}"
        );
        let arn = body["datasetArn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:personalize:{REGION}:{ACCOUNT}:dataset/casing-recreate-dsg/INTERACTIONS"
            ),
            "case {case:?}: expected the type's own ARN, got {arn:?}"
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
            Some("INTERACTIONS"),
            "case {case:?}: expected the type's upper-case name, got {:?}",
            dataset["datasetType"]
        );
        // The dataset's NAME, a free string with no documented value set, IS echoed
        // character for character — so the type's spelling is not a side effect of
        // rewriting every member.
        assert_eq!(
            dataset["name"].as_str(),
            Some("casing-recreate-ds"),
            "case {case:?}: expected the created name echoed verbatim, got {dataset:?}"
        );

        let (status, body) = emulator
            .call_with_arn("DeleteDataset", "datasetArn", &arn)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteDataset to answer 200, got {status} {body:?}"
        );
    }
}
