// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage for the `PackageVersionStatus` INPUT set — the statuses a
//! request may supply, as opposed to the wider set a response may report.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is asserted here is the wire behaviour a
//! CodeArtifact client sees — status, error code, message and body.
//!
//! The rules under test, each asserted over a TABLE rather than one example:
//!
//! 1. A status a request SUPPLIES — `ListPackageVersions`' `status` filter, and the
//!    `expectedStatus` precondition of `DeletePackageVersions`, `DisposePackageVersions`
//!    and `UpdatePackageVersionsStatus` — must name a state a stored version can be in.
//!    `PackageVersionStatus` is also what responses report, so its catalog is wider: the
//!    value no record ever carries is refused `ValidationException` (400) with the message
//!    `<value> is not a valid input status`, and every other value is accepted and keeps
//!    filtering or pinning as before.
//! 2. The refusal is the whole answer: a refused request changes nothing a later caller
//!    sees — the addressed version keeps the status it had.
//! 3. The refusal is decided from the REQUEST, so it precedes the coordinate lookup: the
//!    same 400 is answered for a domain, repository or package this emulator does not
//!    hold, where the same request with an acceptable status is the 404 that names the
//!    missing resource.
//! 4. The accepted set is exactly the modelled catalog minus the one value no stored
//!    version carries — asserted against the generated model catalog, so a model that
//!    grows a status fails here rather than silently widening what a request may supply.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The status catalog comes from the model's own generated constant, and the input subset
// from the one place the handlers share it, so neither list is hand-copied here.
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::ops::types::PACKAGE_VERSION_STATUS_VALUES;
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::INPUT_PACKAGE_VERSION_STATUSES;

// The HTTP harness the crate's suites share, the publish helpers from the suite that owns
// `PublishPackageVersion`, and the two per-version request builders from the suite that
// owns the status updates.
use crate::harness::{call, Answer};
use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};
use crate::update_operations_persist::{dispose_versions, update_versions_status};

/// The status a publish stores, which is what every row here starts from.
const PUBLISHED: &str = "Published";

/// The `targetStatus` the `UpdatePackageVersionsStatus` rows move to — one of the three
/// statuses that operation accepts there, so a row's outcome turns on `expectedStatus`
/// alone.
const ARCHIVED: &str = "Archived";

/// One operation that takes a `PackageVersionStatus` as input, named by the member it
/// takes it in. Every variant sends the SAME status value through a different member, so a
/// refusal implemented in one handler and forgotten in another cannot pass.
#[derive(Debug, Clone, Copy)]
enum StatusInput {
    /// `ListPackageVersions$status` — a filter over the versions the package holds.
    ListFilter,
    /// `DeletePackageVersions$expectedStatus` — a precondition on each named version.
    DeleteExpected,
    /// `DisposePackageVersions$expectedStatus`.
    DisposeExpected,
    /// `UpdatePackageVersionsStatus$expectedStatus`.
    UpdateExpected,
}

impl StatusInput {
    /// Every member under test, for a table that spans them.
    const ALL: [StatusInput; 4] = [
        StatusInput::ListFilter,
        StatusInput::DeleteExpected,
        StatusInput::DisposeExpected,
        StatusInput::UpdateExpected,
    ];
}

/// `ListPackageVersions` for the package a [`Publish`] addresses, with the optional
/// `status` filter the model binds to the query string.
async fn list_versions(
    router: &MultiServiceRouter,
    request: &Publish,
    status: Option<&str>,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    if let Some(status) = status {
        uri.push_str(&format!("&status={status}"));
    }
    call(router, "POST", &uri, Some(json!({}))).await
}

/// `DeletePackageVersions` for one version of the package a [`Publish`] addresses: the
/// version list and the optional `expectedStatus` travel in the body.
async fn delete_versions(
    router: &MultiServiceRouter,
    request: &Publish,
    versions: &[&str],
    expected_status: Option<&str>,
) -> Answer {
    let mut uri = format!(
        "/v1/package/versions/delete?domain={}&repository={}&format={}&package={}",
        request.domain, request.repository, request.format, request.package
    );
    if let Some(namespace) = &request.namespace {
        uri.push_str(&format!("&namespace={namespace}"));
    }
    let mut body = json!({ "versions": versions });
    if let Some(expected) = expected_status {
        body["expectedStatus"] = json!(expected);
    }
    call(router, "POST", &uri, Some(body)).await
}

/// Send `status` through the member `input` names, against the package `request`
/// addresses.
async fn send(
    router: &MultiServiceRouter,
    request: &Publish,
    input: StatusInput,
    status: &str,
) -> Answer {
    let version = request.version.clone();
    match input {
        StatusInput::ListFilter => list_versions(router, request, Some(status)).await,
        StatusInput::DeleteExpected => {
            delete_versions(router, request, &[version.as_str()], Some(status)).await
        }
        StatusInput::DisposeExpected => {
            dispose_versions(router, request, &[version.as_str()], Some(status), None).await
        }
        StatusInput::UpdateExpected => {
            update_versions_status(
                router,
                request,
                &[version.as_str()],
                ARCHIVED,
                Some(status),
                None,
            )
            .await
        }
    }
}

/// The message a supplied status outside the input set owes: the value as it arrived, then
/// the fixed text.
fn refusal_message(status: &str) -> String {
    format!("{status} is not a valid input status")
}

/// A router holding one published version, and the [`Publish`] that addresses it.
async fn emulator_with_version(package: &str) -> (MultiServiceRouter, Publish) {
    let router = emulator_with_repository().await;
    let request = Publish::new(package, "1.0.0", "payload.bin", binary_content(48, 7));
    let answer = publish(&router, &request).await;
    assert_eq!(
        (answer.status, answer.code()),
        (200, String::new()),
        "publishing {package} 1.0.0 must succeed, got {answer:?}"
    );
    (router, request)
}

/// The `status` `DescribePackageVersion` reports for the version a [`Publish`] addresses.
async fn described_status(router: &MultiServiceRouter, request: &Publish) -> Option<String> {
    describe(router, request).await.body["packageVersion"]["status"]
        .as_str()
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the whole status catalog, through every input member.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_status_input_accepts_only_a_status_a_version_can_be_in() {
    // Rows span both dimensions the rule generalises over: the member the status arrives
    // in, and the value itself (every value the model declares, accepted and refused).
    for input in StatusInput::ALL {
        for status in PACKAGE_VERSION_STATUS_VALUES {
            let case = format!("{input:?} with {status}");
            // One emulator per row: a row that is ACCEPTED may delete or dispose the
            // version it names, so no row inherits what another one did.
            let (router, request) = emulator_with_version("input-status").await;
            let answer = send(&router, &request, input, status).await;

            if INPUT_PACKAGE_VERSION_STATUSES.contains(status) {
                assert_eq!(
                    (answer.status, answer.code()),
                    (200, String::new()),
                    "case {case:?}: a status a stored version can be in must be accepted, \
                     got {answer:?}"
                );
                continue;
            }

            assert_eq!(
                (answer.status, answer.code()),
                (400, "ValidationException".to_string()),
                "case {case:?}: a status no stored version can be in must be refused, got \
                 {answer:?}"
            );
            assert_eq!(
                answer.message(),
                refusal_message(status),
                "case {case:?}: expected {:?}, got {answer:?}",
                refusal_message(status)
            );
            // Rule 2: the refusal is the whole answer.
            let reported = described_status(&router, &request).await;
            assert_eq!(
                reported.as_deref(),
                Some(PUBLISHED),
                "case {case:?}: a refused request must leave the version at {PUBLISHED}, got \
                 {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_status_keeps_filtering_and_pinning() {
    // The positive control for rule 1, one row per accepted status: the filter reports the
    // versions carrying it (here only the published one), and the `expectedStatus`
    // precondition decides PER VERSION rather than failing the call.
    let (router, request) = emulator_with_version("accepted-status").await;
    for status in INPUT_PACKAGE_VERSION_STATUSES {
        let answer = list_versions(&router, &request, Some(*status)).await;
        let listed: Vec<String> = answer.body["versions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["version"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = if *status == PUBLISHED {
            vec![request.version.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {status:?}: the filtered listing must answer 200, got {answer:?}"
        );
        assert_eq!(
            listed, expected,
            "case {status:?}: expected {expected:?}, got {answer:?}"
        );

        let answer = update_versions_status(
            &router,
            &request,
            &[request.version.as_str()],
            PUBLISHED,
            Some(*status),
            None,
        )
        .await;
        assert_eq!(
            (answer.status, answer.code()),
            (200, String::new()),
            "case {status:?}: an expectedStatus a version can be in is decided per version, \
             got {answer:?}"
        );
        let decided = if *status == PUBLISHED {
            "successfulVersions"
        } else {
            "failedVersions"
        };
        assert!(
            answer.body[decided]
                .as_object()
                .is_some_and(|map| map.contains_key(request.version.as_str())),
            "case {status:?}: the version must be reported in {decided}, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the refusal is decided before the coordinate is resolved.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_status_outranks_an_unresolvable_coordinate() {
    // The one value outside the input set, sent against coordinates this emulator does not
    // hold. Rows differ along WHICH step of the coordinate is missing.
    let refused: Vec<&str> = PACKAGE_VERSION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !INPUT_PACKAGE_VERSION_STATUSES.contains(status))
        .collect();

    let (router, held) = emulator_with_version("unresolvable").await;
    let missing_domain = {
        let mut request = held.clone();
        request.domain = "no-such-domain".to_string();
        ("an unknown domain", request)
    };
    let missing_repository = {
        let mut request = held.clone();
        request.repository = "no-such-repo".to_string();
        ("an unknown repository", request)
    };
    let missing_package = {
        let mut request = held.clone();
        request.package = "no-such-package".to_string();
        ("an unknown package", request)
    };

    for (coordinate, request) in [missing_domain, missing_repository, missing_package] {
        for input in StatusInput::ALL {
            for status in &refused {
                let case = format!("{input:?} with {status} against {coordinate}");
                let answer = send(&router, &request, input, status).await;
                assert_eq!(
                    (answer.status, answer.code()),
                    (400, "ValidationException".to_string()),
                    "case {case:?}: the bad request outranks the missing resource, got \
                     {answer:?}"
                );
                assert_eq!(
                    answer.message(),
                    refusal_message(status),
                    "case {case:?}: expected {:?}, got {answer:?}",
                    refusal_message(status)
                );
            }

            // The control: the SAME unresolvable coordinate with a status a request may
            // supply is the 404 that names the missing resource, so the rows above are not
            // passing because the coordinate is wrong.
            let case = format!("{input:?} with {PUBLISHED} against {coordinate}");
            let answer = send(&router, &request, input, PUBLISHED).await;
            assert_eq!(
                (answer.status, answer.code()),
                (404, "ResourceNotFoundException".to_string()),
                "case {case:?}: an acceptable status still resolves the coordinate, got \
                 {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the input set against the model's own catalog.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_input_set_is_the_model_catalog_minus_the_unstorable_status() {
    // Source: the generated `PackageVersionStatus` catalog. Every input status must be one
    // of its values, and the values it holds that are NOT input statuses are exactly the
    // ones no stored version carries — `Deleted`, which only the call that removed a
    // version reports.
    for status in INPUT_PACKAGE_VERSION_STATUSES {
        assert!(
            PACKAGE_VERSION_STATUS_VALUES.contains(status),
            "case {status:?}: an input status must be a modelled PackageVersionStatus, the \
             catalog is {PACKAGE_VERSION_STATUS_VALUES:?}"
        );
    }
    let report_only: Vec<&str> = PACKAGE_VERSION_STATUS_VALUES
        .iter()
        .copied()
        .filter(|status| !INPUT_PACKAGE_VERSION_STATUSES.contains(status))
        .collect();
    assert_eq!(
        report_only,
        vec!["Deleted"],
        "the model grew a status this split does not place, got {report_only:?}"
    );

    // And the split is observable on the wire, not just in the constants: the reporting
    // call is what puts that value in front of a client.
    let (router, request) = emulator_with_version("report-only").await;
    let answer = delete_versions(&router, &request, &[request.version.as_str()], None).await;
    assert_eq!(
        answer.body["successfulVersions"][request.version.as_str()]["status"].as_str(),
        Some(report_only[0]),
        "the delete must REPORT the status no request may supply, got {answer:?}"
    );
    let answer = list_versions(&router, &request, Some(report_only[0])).await;
    assert_eq!(
        (answer.status, answer.code()),
        (400, "ValidationException".to_string()),
        "the same value must still be refused as an input, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("versions").cloned().unwrap_or(Value::Null),
        Value::Null,
        "a refused listing carries no versions, got {answer:?}"
    );
}
