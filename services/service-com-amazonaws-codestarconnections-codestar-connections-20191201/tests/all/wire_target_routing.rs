// SPDX-License-Identifier: CC0-1.0

//! The host READ is reached under every `X-Amz-Target` prefix form a client writes,
//! including the one the recorded traffic for this service carries.
//!
//! RULE: on the awsJson protocols a request names its operation out of band, in
//! `x-amz-target: <target prefix>.<OperationName>`. The prefix is opaque text an SDK
//! copies out of its service model and may carry dots of its own — this service's
//! published prefix is the fully qualified Coral service name,
//! `com.amazonaws.codestar.connections.CodeStar_connections_20191201` — so the operation
//! is the segment after the LAST dot, whatever stands in front of it.
//!
//! `sdk_target_dispatch.rs` pins that rule over the whole operation surface and over a
//! CONNECTION round trip. This suite pins the host half of it: the same host record is
//! reported, and the same absence refused, whichever prefix form named `GetHost` — so the
//! read a client of the real service performs (which sends the dotted form) and the read
//! the crate's other suites perform (which send the bare shape name) are one operation
//! over one store.
//!
//! Nothing here is restated: the prefix forms are `sdk_target_dispatch.rs`'s table, the
//! harness and the envelope readers are `connection_lifecycle.rs`'s, and the rows, the
//! seeding and the assertions are `host_reads.rs`'s. Dispatch does not vary per row, so a
//! few representative rows carry it: a host with a VPC configuration in a transitional
//! status, and one with neither.

use serde_json::json;

use super::connection_lifecycle::{error_code, error_message, Emulator};
use super::host_reads::{
    assert_reports_row, host_arn, host_rows, seed_host, seed_rows, stored_hosts, HostRow,
};
use super::sdk_target_dispatch::PREFIXES;

/// The rows this suite drives: the two the table opens with, which differ in status, in
/// whether they carry a VPC configuration and in whether they carry a status description —
/// enough to tell a reported record from a fabricated one behind each prefix form.
fn representative_rows() -> Vec<HostRow> {
    host_rows().into_iter().take(2).collect()
}

/// RULE: one stored host is reported identically behind every prefix form that names this
/// service, and reading it behind any of them changes nothing a later caller sees.
#[tokio::test]
async fn a_stored_host_is_reported_behind_every_prefix_form() {
    let emulator = Emulator::new();
    let rows = representative_rows();
    let arns = seed_rows(&emulator, &rows);
    let before = stored_hosts(&emulator);

    for (form, prefix) in PREFIXES {
        for (row, arn) in rows.iter().zip(&arns) {
            let case = format!("{form} / {} ({})", row.case, row.name);
            let (status, response) = emulator
                .call_target(&format!("{prefix}.GetHost"), json!({ "HostArn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for a stored host, got {status} {response:?}"
            );
            assert_reports_row(&case, row, &response);
        }
    }

    assert_eq!(
        stored_hosts(&emulator),
        before,
        "reading hosts behind the prefix forms changed what the store holds"
    );
}

/// RULE: a `HostArn` that resolves to no host is the modelled absence behind every prefix
/// form too — the refusal is the handler's answer about the resource, never the
/// dispatcher's about the operation.
#[tokio::test]
async fn an_absent_host_is_refused_behind_every_prefix_form() {
    let emulator = Emulator::new();
    // One host exists, so the refusal comes from the lookup and not from an empty store.
    let kept = seed_host(
        &emulator,
        "kept",
        &HostRow {
            case: "the kept host",
            name: "kept-host".to_string(),
            provider_type: "GitHubEnterpriseServer",
            provider_endpoint: "https://kept.example.internal".to_string(),
            status: "AVAILABLE",
            status_message: None,
            vpc_configuration: None,
        },
    );
    let absent = host_arn("00000000-0000-0000-0000-000000000000");
    let before = stored_hosts(&emulator);

    for (form, prefix) in PREFIXES {
        let (status, response) = emulator
            .call_target(&format!("{prefix}.GetHost"), json!({ "HostArn": &absent }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {form:?}: expected the modelled absence for {absent:?}, got \
             {status} {response:?}"
        );
        assert!(
            error_message(&response).contains(&absent),
            "case {form:?}: the refusal must name the ARN supplied, got {:?}",
            error_message(&response)
        );

        // The host that IS stored still reports behind the same form.
        let (status, response) = emulator
            .call_target(&format!("{prefix}.GetHost"), json!({ "HostArn": &kept }))
            .await;
        assert_eq!(
            status, 200,
            "case {form:?}: the stored host must still resolve, got {status} {response:?}"
        );
    }

    assert_eq!(
        stored_hosts(&emulator),
        before,
        "a refused read changed what the store holds"
    );
}
