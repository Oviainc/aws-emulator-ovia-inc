// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` body every ARN-addressed operation of this service
//! answers an export ARN that resolves to nothing with.
//!
//! The rule under test: the not-found `Message` is ONE fixed, request-independent string.
//! It does not restate — and does not vary with — the requested ARN, its partition, its
//! region, its account id or its resource name, so the reply to "this export does not
//! exist in your account" carries the same message as the reply to "this ARN names a
//! different account", and nothing in the message confirms anything about the ARN the
//! caller supplied. The identity of the thing that was not found is reported in the
//! modelled `ResourceId` member ("The identifier of the resource that was not found" —
//! smithy_query shape `ResourceNotFoundException`, member `ResourceId`, required), which
//! is where the model puts it.
//!
//! It is asserted over a TABLE of well-formed export ARNs that differ along everything
//! the rule generalises over — partition, region (including the empty region the modelled
//! `Arn` pattern permits), account id (the emulator's own and several foreign ones),
//! resource-name length from the shortest the length constraint allows to a long
//! multi-segment one, and the punctuation the pattern's resource class allows — crossed
//! with every operation that takes such an ARN, and with the request's own signing
//! region. The recorded cases from the issue are two rows of that table, not the
//! specification.
//!
//! The ARNs all satisfy the modelled `Arn` constraints (`@length 20..2048`, pattern
//! `^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$` —
//! smithy_query shape `Arn`), so each 404 comes from the handler's lookup and not from
//! boundary validation.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::EXPORT_NOT_FOUND_MESSAGE;

use crate::export_lifecycle::{cases, error_code, Client};

/// `GenericString`'s ceiling, which `ResourceNotFoundException.Message` targets:
/// `@length 0..1024` (smithy_query shape `ResourceNotFoundException`, member `Message`).
const MESSAGE_MAX_LEN: usize = 1024;

/// One row: a well-formed export ARN that names nothing, and the region the request
/// itself is signed for.
#[derive(Debug, Clone)]
struct Row {
    label: String,
    arn: String,
    /// The credential-scope region of the call, so the table also covers a request whose
    /// own region differs from the one in the ARN it asks about.
    signing_region: &'static str,
}

/// The table of absent ARNs.
fn rows() -> Vec<Row> {
    let own = ::engine::emulator_account_id();
    // The shortest resource segment the pattern allows is a single character; with an
    // empty region the whole ARN is still 40 characters, above the modelled minimum 20.
    let shortest = format!("arn:aws:bcm-data-exports::{own}:a");
    // A long, multi-segment resource name using every punctuation class the pattern's
    // resource segment allows (`-`, `/`, `:`, `_`) and staying under the 2048 ceiling.
    let long_tail = "segment_one/segment-two:segment3/".repeat(30);
    let longest = format!("arn:aws:bcm-data-exports:eu-central-1:{own}:export/{long_tail}end");

    let mut rows = vec![
        // The two recorded cases: an absent export in the caller's own account, and the
        // same ARN shape naming a different account.
        Row {
            label: "own-account absent export".to_string(),
            arn: format!(
                "arn:aws:bcm-data-exports:us-east-1:{own}:export/does-not-exist/00000000-0000-0000-0000-000000000000"
            ),
            signing_region: "us-east-1",
        },
        Row {
            label: "foreign account 111122223333".to_string(),
            arn: "arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test/00000000-0000-0000-0000-000000000000".to_string(),
            signing_region: "us-east-1",
        },
        // Rows the issue never cited.
        Row {
            label: "foreign account, different region, request signed elsewhere".to_string(),
            arn: "arn:aws:bcm-data-exports:ap-southeast-2:210987654321:export/other_tenant"
                .to_string(),
            signing_region: "eu-west-1",
        },
        Row {
            label: "gov partition".to_string(),
            arn: "arn:aws-us-gov:bcm-data-exports:us-gov-west-1:444455556666:export/absent"
                .to_string(),
            signing_region: "us-gov-west-1",
        },
        Row {
            label: "china partition".to_string(),
            arn: "arn:aws-cn:bcm-data-exports:cn-north-1:777788889999:export/absent".to_string(),
            signing_region: "cn-north-1",
        },
        Row {
            label: "empty region, shortest resource name".to_string(),
            arn: shortest,
            signing_region: "us-east-1",
        },
        Row {
            label: "long multi-segment resource name".to_string(),
            arn: longest,
            signing_region: "eu-central-1",
        },
    ];
    // Account id is the member the cross-tenant check varies, so walk a few more of them
    // mechanically: the message must not move for any of them.
    for account in ["000000000001", "123456789012", "999999999999"] {
        rows.push(Row {
            label: format!("account {account}"),
            arn: format!("arn:aws:bcm-data-exports:us-west-2:{account}:export/absent_export"),
            signing_region: "us-west-2",
        });
    }
    rows
}

/// Every request that hands an ARN to this service, for the ARN in `row`.
///
/// `ListTagsForResource` is included only for an ARN in the caller's OWN account: the
/// tagging path validates the ARN's account BEFORE any lookup and refuses an ARN of
/// another account with the modelled `ValidationException` instead of looking it up at
/// all (`tests/all/tagging_arn_validation.rs` is where that rule is asserted), so a
/// foreign-account ARN never reaches the lookup THIS rule is about.
fn requests(row: &Row, export: &Value) -> Vec<(&'static str, Value)> {
    let mut requests = vec![
        ("GetExport", json!({ "ExportArn": row.arn })),
        ("DeleteExport", json!({ "ExportArn": row.arn })),
        (
            "UpdateExport",
            json!({ "ExportArn": row.arn, "Export": export }),
        ),
    ];
    if names_own_account(&row.arn) {
        requests.push((
            "ListTagsForResource",
            json!({ "ResourceArn": row.arn }),
        ));
    }
    requests
}

/// Whether `arn`'s account segment is this emulator's one account.
fn names_own_account(arn: &str) -> bool {
    arn.split(':').nth(4) == Some(::engine::emulator_account_id())
}

/// Assert the 404 for `row`/`operation` and return the message it carried.
fn check_not_found(label: &str, arn: &str, body: &Value, status: u16) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {label:?}: expected HTTP 404 ResourceNotFoundException, got {status} with {body}"
    );
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: the 404 must carry a Message, got {body}"))
        .to_string();
    assert_eq!(
        message, EXPORT_NOT_FOUND_MESSAGE,
        "case {label:?}: expected the fixed not-found message {EXPORT_NOT_FOUND_MESSAGE:?}, got {message:?}"
    );
    assert!(
        message.len() <= MESSAGE_MAX_LEN,
        "case {label:?}: the message must satisfy the modelled length 0..{MESSAGE_MAX_LEN}, got {} characters in {message:?}",
        message.len()
    );
    // Nothing of the request may appear in the message: not the ARN, and not any of its
    // distinctive segments (partition, region, account, resource name). Segments shorter
    // than this are not distinguishable from ordinary English and are skipped.
    const DISTINCTIVE_LEN: usize = 8;
    for segment in std::iter::once(arn).chain(
        arn.split(':')
            .filter(|segment| segment.len() >= DISTINCTIVE_LEN),
    ) {
        assert!(
            !message.contains(segment),
            "case {label:?}: the message must not restate request content, but {message:?} contains {segment:?}"
        );
    }
    // The identity belongs in the modelled ResourceId member instead.
    assert_eq!(
        body["ResourceId"],
        json!(arn),
        "case {label:?}: the 404 must report the ARN it could not find in ResourceId, got {body}"
    );
    message
}

/// Over the whole table, and for every ARN-addressed operation, the not-found message is
/// the same fixed string.
#[tokio::test]
async fn not_found_message_is_a_fixed_constant_for_every_absent_arn() {
    let client = Client::new();
    let export = cases().remove(0).create_request()["Export"].clone();

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in rows() {
        for (operation, request) in requests(&row, &export) {
            let label = format!("{} / {operation}", row.label);
            let (status, body) = client
                .call_in_region(row.signing_region, operation, &request)
                .await;
            seen.insert(check_not_found(&label, &row.arn, &body, status));
        }
    }

    assert_eq!(
        seen.len(),
        1,
        "every absent ARN must get the SAME message, but the table produced {seen:?}"
    );
}

/// The constant is the answer for a genuine absence too, not a blanket 404: an export
/// that exists is read back, and the same ARN answers the fixed message once deleted.
#[tokio::test]
async fn an_existing_export_is_found_and_answers_the_constant_once_deleted() {
    let client = Client::new();
    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // Round trip: while it exists, the reader finds it.
    let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        fetched["Export"]["Name"],
        case.create_request()["Export"]["Name"],
        "GetExport must report the export CreateExport stored, got {fetched}"
    );
    let tags = client
        .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
        .await;
    assert!(
        tags["ResourceTags"].is_array(),
        "ListTagsForResource must answer an existing export, got {tags}"
    );

    client.ok("DeleteExport", &json!({ "ExportArn": arn })).await;

    // And now the very ARN the service minted itself gets the same fixed message as a
    // never-created or foreign-account one.
    for (operation, request) in [
        ("GetExport", json!({ "ExportArn": arn })),
        ("DeleteExport", json!({ "ExportArn": arn })),
        ("ListTagsForResource", json!({ "ResourceArn": arn })),
    ] {
        let (status, body) = client.call(operation, &request).await;
        let label = format!("deleted export / {operation}");
        check_not_found(&label, &arn, &body, status);
    }
}
