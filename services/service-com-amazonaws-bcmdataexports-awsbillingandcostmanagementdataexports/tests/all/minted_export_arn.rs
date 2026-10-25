// SPDX-License-Identifier: CC0-1.0

//! The identity the server-minted `ExportArn` carries, over the awsJson1_1 wire.
//!
//! The rule under test: the ARN this service mints for an export carries a SERVER-GENERATED
//! unique id segment after the caller's `Export.Name`, so the ARN is not a function of the
//! name and every created export — including the one that takes a name a deleted export
//! held — is addressed by an ARN of its own, which is never reused and which every reader
//! echoes unchanged. The caller's name is unique only among the exports a region currently
//! holds; the ARN identifies the export itself.
//!
//! What a client does with that: it correlates, caches and deduplicates by ARN. An ARN
//! recomputed from the name would make a recreated export indistinguishable from the one
//! that was deleted, so a stale ARN would silently address the new resource.
//!
//! It is asserted over a TABLE whose rows differ along what the rule generalises over: the
//! name's length (one glyph, several, and the longest the model allows) and its glyph
//! classes (digits, upper and lower case, `-`, `_`), the region the create is signed for
//! (differing in length and hyphen count), and how many times the name is created, deleted
//! and created again. The ids themselves are compared ACROSS rows, so no two exports of the
//! whole table may share one.

use serde_json::{json, Value};

use crate::export_lifecycle::{
    cases, error_code, generated_export_id, is_uuid_shaped, Case, Client, NOT_FOUND_STATUS,
};

/// `Export.Name` is `length 1..128`, `pattern ^[0-9A-Za-z\-_]+$` (smithy_query shape
/// `Export`, member `Name`): the rows below span those glyph classes, and no name can carry
/// a `/`, which is why the generated id is the one segment that follows the name.
const NAME_MAX_LEN: usize = 128;

/// One row of the table.
struct Row {
    label: String,
    /// The `Export.Name` the row creates under.
    name: String,
    /// The region the row's calls are signed for, and therefore the ARN's region segment.
    region: &'static str,
    /// How many times the row creates the name, deletes it and creates it again.
    rounds: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a one-glyph name".to_string(),
            name: "a".to_string(),
            region: "us-east-1",
            rounds: 3,
        },
        Row {
            label: "a name of digits only".to_string(),
            name: "0123456789".to_string(),
            region: "eu-west-1",
            rounds: 2,
        },
        Row {
            label: "a name carrying both punctuation classes and both cases".to_string(),
            name: "Mixed-Name_01".to_string(),
            region: "ap-southeast-2",
            rounds: 2,
        },
        Row {
            label: "the longest name the model allows".to_string(),
            name: format!("long_{}{}", "A1-b".repeat((NAME_MAX_LEN - 8) / 4), "x_9"),
            region: "cn-north-1",
            rounds: 2,
        },
        Row {
            label: "a region with no hyphen".to_string(),
            name: "local_region_export".to_string(),
            region: "local",
            rounds: 2,
        },
    ]
}

/// The row's `CreateExport` request: a definition from the lifecycle table under this row's
/// name, so the definition is one the service really accepts and only the name varies.
fn create_request(row: &Row) -> Value {
    assert!(
        (1..=NAME_MAX_LEN).contains(&row.name.len()),
        "case {:?}: the row's name must satisfy the modelled length 1..{NAME_MAX_LEN}, got {}",
        row.label,
        row.name.len()
    );
    let mut case: Case = cases().remove(0);
    case.set_name(&row.name);
    case.create_request()
}

/// Call `operation` in `region`, requiring HTTP 200.
async fn ok_in(client: &Client, label: &str, region: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = client.call_in_region(region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: {operation} must be served, got {status} with {parsed}"
    );
    parsed
}

/// RULE, first half: every minted ARN is `…:export/<Name>/<generated id>`, the id is the
/// server's own, no two exports share one, and every reader echoes the same ARN.
#[tokio::test]
async fn a_minted_export_arn_carries_a_per_export_generated_id() {
    let client = Client::new();
    let mut ids: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let label = row.label.clone();
        let created = ok_in(
            &client,
            &label,
            row.region,
            "CreateExport",
            &create_request(&row),
        )
        .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateExport returns an ExportArn: {created}"))
            .to_string();

        // The resource portion is the export name followed by ONE further segment, the
        // generated id, which is the server's and not anything the request carried.
        let id = generated_export_id(&label, &arn, &row.name).to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {label:?}: expected a UUID-shaped generated id, got {id:?}"
        );
        assert_eq!(
            arn.split(':').nth(3),
            Some(row.region),
            "case {label:?}: the ARN must carry the call's region, got {arn:?}"
        );

        // Every reader reports the ARN the create minted, unchanged — and reading it twice
        // reports the same one, so no read re-mints an identity.
        let fetched = ok_in(
            &client,
            &label,
            row.region,
            "GetExport",
            &json!({ "ExportArn": arn }),
        )
        .await;
        assert_eq!(
            fetched["Export"]["ExportArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: GetExport must report the minted ARN, got {fetched}"
        );
        assert_eq!(
            fetched["Export"]["Name"].as_str(),
            Some(row.name.as_str()),
            "case {label:?}: GetExport must report the created name, got {fetched}"
        );
        let again = ok_in(
            &client,
            &label,
            row.region,
            "GetExport",
            &json!({ "ExportArn": arn }),
        )
        .await;
        assert_eq!(
            again["Export"]["ExportArn"], fetched["Export"]["ExportArn"],
            "case {label:?}: a second read must report the same ARN, got {again}"
        );
        let listed = ok_in(&client, &label, row.region, "ListExports", &json!({})).await;
        let listed_arns: Vec<&str> = listed["Exports"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: ListExports returns Exports: {listed}"))
            .iter()
            .filter_map(|reference| reference["ExportArn"].as_str())
            .collect();
        assert!(
            listed_arns.contains(&arn.as_str()),
            "case {label:?}: ListExports must carry the minted ARN, got {listed_arns:?}"
        );

        // An update replaces the definition, not the identity: the export keeps the ARN it
        // was addressed by, generated id and all.
        let mut updated_case: Case = cases().remove(0);
        updated_case.set_name(&row.name);
        let updated = ok_in(
            &client,
            &label,
            row.region,
            "UpdateExport",
            &json!({ "ExportArn": arn, "Export": updated_case.update_export() }),
        )
        .await;
        assert_eq!(
            updated["ExportArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: UpdateExport must report the ARN it updated, got {updated}"
        );

        ids.push((label, id));
    }

    // The id is PER EXPORT: no two exports of the table share one, however alike their
    // names, regions or definitions.
    for (index, (label, id)) in ids.iter().enumerate() {
        let shared = ids
            .iter()
            .enumerate()
            .find(|(other, (_, candidate))| *other != index && candidate == id);
        assert!(
            shared.is_none(),
            "case {label:?}: the generated id {id:?} is shared with {:?}",
            shared.map(|(_, (other, _))| other)
        );
    }
}

/// RULE, second half: a name freed by a delete is reusable, and the export that takes it is
/// a NEW export with a new ARN — while a name a live export holds cannot be created again.
#[tokio::test]
async fn a_name_created_again_after_a_delete_is_a_new_export_with_a_new_arn() {
    for row in rows() {
        // One client per row, so each row's region and name start from an empty store.
        let client = Client::new();
        let mut minted: Vec<String> = Vec::new();

        for round in 0..row.rounds {
            let label = format!("{} round {round}", row.label);
            let created = ok_in(
                &client,
                &label,
                row.region,
                "CreateExport",
                &create_request(&row),
            )
            .await;
            let arn = created["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: CreateExport returns an ExportArn"))
                .to_string();
            generated_export_id(&label, &arn, &row.name);
            assert!(
                !minted.contains(&arn),
                "case {label:?}: the ARN {arn:?} was already minted for an earlier export"
            );

            // While this export holds the name, the name is taken: the refusal is by NAME,
            // so it does not depend on the generated id in the ARN just minted.
            let (status, body) = client
                .call_in_region(row.region, "CreateExport", &create_request(&row))
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?}: a name a live export holds must be refused, got {body}"
            );

            // Each earlier ARN stays dead: it was never reused, so it addresses nothing.
            for earlier in &minted {
                let (status, body) = client
                    .call_in_region(row.region, "GetExport", &json!({ "ExportArn": earlier }))
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                    "case {label:?}: the ARN {earlier:?} of a deleted export must name \
                     nothing, got {body}"
                );
            }

            ok_in(
                &client,
                &label,
                row.region,
                "DeleteExport",
                &json!({ "ExportArn": arn }),
            )
            .await;
            minted.push(arn);
        }

        assert_eq!(
            minted.len(),
            row.rounds,
            "case {:?}: every round must have minted an ARN of its own, got {minted:?}",
            row.label
        );
    }
}
