// SPDX-License-Identifier: CC0-1.0

//! How a publish into a version the repository ALREADY holds is refused, over the wire.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is pinned here is the status, the error code and
//! the `message` a CodeArtifact client reads.
//!
//! THE RULE: a publish into a package version the repository already holds in a status other
//! than `Unfinished` is refused with `ConflictException` (HTTP 409;
//! `smithy_query(query="operation",operation="PublishPackageVersion")` lists
//! `ConflictException` httpError 409), and the refusal IDENTIFIES what it refused and why:
//! the package FORMAT, the whole `<namespace>/<package>/<version>` path the version is
//! addressed by, the status the stored version is in, and the rule that only an `Unfinished`
//! version can still be updated ("Once a package version’s status is set to Published, it
//! cannot change back to Unfinished",
//! `smithy_query(query="doc",shape="PublishPackageVersion")`). A version that IS `Unfinished`
//! takes the publish instead of refusing it, which is the other half of the same rule.
//!
//! The table's rows differ along what the rule generalises over: the status the stored
//! version is in (every status a stored version can be observed in, reached through the
//! operation that owns it), the namespace and package name the version is addressed by, and
//! the version string.

use smithy_server_runtime::MultiServiceRouter;
use service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::state::{
    PACKAGE_VERSION_STATUS_ARCHIVED, PACKAGE_VERSION_STATUS_DISPOSED,
    PACKAGE_VERSION_STATUS_PUBLISHED, PACKAGE_VERSION_STATUS_UNFINISHED,
    PACKAGE_VERSION_STATUS_UNLISTED,
};

use crate::package_version_publish::{
    binary_content, describe, emulator_with_repository, publish, Publish,
};
use crate::update_operations_persist::{dispose_versions, update_versions_status};

/// One stored version to publish over: how it is addressed and the status it is left in.
#[derive(Debug, Clone, Copy)]
struct Row {
    namespace: &'static str,
    package: &'static str,
    version: &'static str,
    /// The status the stored version carries when the second publish arrives.
    status: &'static str,
}

/// The statuses a stored version can be observed in, each reached through the operation that
/// owns it: a publish stores `Published` (or `Unfinished` while it is open),
/// `UpdatePackageVersionsStatus` moves it to `Unlisted` or `Archived`, and
/// `DisposePackageVersions` commits it to `Disposed`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            namespace: "team-core",
            package: "conflict-published",
            version: "1.0.0",
            status: PACKAGE_VERSION_STATUS_PUBLISHED,
        },
        Row {
            namespace: "team-core",
            package: "conflict-unlisted",
            version: "2.3.4",
            status: PACKAGE_VERSION_STATUS_UNLISTED,
        },
        Row {
            namespace: "org.example.tools",
            package: "conflict-archived",
            version: "0.0.1-alpha.1",
            status: PACKAGE_VERSION_STATUS_ARCHIVED,
        },
        Row {
            namespace: "sim",
            package: "conflict.disposed_pkg",
            version: "10.20.30",
            status: PACKAGE_VERSION_STATUS_DISPOSED,
        },
    ]
}

/// The request that addresses this row's version.
fn request(row: &Row) -> Publish {
    Publish::new(row.package, row.version, "payload.bin", binary_content(64, 7))
        .namespace(row.namespace)
}

/// The whole sentence the refusal must state, built the way a client would build it: from
/// the request's own format and coordinate and from the status the stored version is in.
fn expected_message(row: &Row) -> String {
    format!(
        "Cannot update existing generic package version '{}/{}/{}' with status '{}'. Only \
         '{PACKAGE_VERSION_STATUS_UNFINISHED}' package versions can be updated.",
        row.namespace, row.package, row.version, row.status
    )
}

/// Leave this row's version stored in the status the row names.
async fn given_version_in_status(router: &MultiServiceRouter, row: &Row) {
    let request = request(row);
    let published = publish(router, &request).await;
    assert_eq!(
        published.status, 200,
        "case {:?}: the first publish must store the version, got {published:?}",
        row.package
    );
    match row.status {
        PACKAGE_VERSION_STATUS_PUBLISHED => {}
        PACKAGE_VERSION_STATUS_DISPOSED => {
            let disposed = dispose_versions(router, &request, &[row.version], None, None).await;
            assert_eq!(
                disposed.status, 200,
                "case {:?}: the dispose must succeed, got {disposed:?}",
                row.package
            );
        }
        target => {
            let moved =
                update_versions_status(router, &request, &[row.version], target, None, None).await;
            assert_eq!(
                moved.status, 200,
                "case {:?}: the move to {target} must succeed, got {moved:?}",
                row.package
            );
        }
    }
    let described = describe(router, &request).await;
    assert_eq!(
        described.body["packageVersion"]["status"].as_str(),
        Some(row.status),
        "case {:?}: the version must be stored in {} before the second publish, got \
         {described:?}",
        row.package,
        row.status
    );
}

/// THE RULE: the refusal names the format, the addressed coordinate, the blocking status and
/// the `Unfinished`-only rule — over every status a stored version can carry.
#[tokio::test]
async fn republishing_a_closed_version_names_the_coordinate_the_status_and_the_rule() {
    let router = emulator_with_repository().await;

    for row in rows() {
        given_version_in_status(&router, &row).await;

        let refused = publish(&router, &request(&row)).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (409, "ConflictException"),
            "case {:?}: expected the modelled conflict, got {refused:?}",
            row.package
        );
        assert_eq!(
            refused.message(),
            expected_message(&row),
            "case {:?}: expected the refusal to name the format, the coordinate, the status \
             and the rule, got {:?}",
            row.package,
            refused.message()
        );
        // The refused publish identifies the version it refused in `resourceId` too, and it
        // changed nothing: the stored version is still in the status it was in.
        assert_eq!(
            refused.body["resourceId"].as_str(),
            Some(format!("{}/{}/{}", row.namespace, row.package, row.version).as_str()),
            "case {:?}: expected the addressed version as the resourceId, got {refused:?}",
            row.package
        );
        let described = describe(&router, &request(&row)).await;
        assert_eq!(
            described.body["packageVersion"]["status"].as_str(),
            Some(row.status),
            "case {:?}: a refused publish must leave the version as it was, got {described:?}",
            row.package
        );
    }
}

/// THE OTHER HALF: an `Unfinished` version is still open, so the identical second publish is
/// taken rather than refused — and closing it makes the next one state the refusal with
/// `Published` as the blocking status.
#[tokio::test]
async fn an_unfinished_version_takes_the_publish_the_closed_one_refuses() {
    let router = emulator_with_repository().await;

    // (case, the asset the second publish carries)
    for (case, asset) in [
        ("the same asset again", "payload.bin"),
        ("a second asset", "extra.bin"),
    ] {
        let package = format!("open-{}", asset.replace('.', "-"));
        let open = Publish::new(&package, "1.0.0", "payload.bin", binary_content(32, 3))
            .namespace("team-core")
            .unfinished(true);
        let first = publish(&router, &open).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first publish must open the version, got {first:?}"
        );

        let again = Publish {
            asset_name: asset.to_string(),
            ..open.clone()
        };
        let second = publish(&router, &again).await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: an unfinished version must take the publish, got {second:?}"
        );
        assert_eq!(
            second.body["status"].as_str(),
            Some(PACKAGE_VERSION_STATUS_UNFINISHED),
            "case {case:?}: the version must stay open, got {second:?}"
        );

        // Closed by a publish that does not ask to keep it open, and then immutable: the
        // refusal states the status that now blocks it.
        let closed = publish(
            &router,
            &Publish {
                unfinished: None,
                ..open.clone()
            },
        )
        .await;
        assert_eq!(
            closed.status, 200,
            "case {case:?}: the closing publish must succeed, got {closed:?}"
        );
        let refused = publish(
            &router,
            &Publish {
                unfinished: None,
                ..open.clone()
            },
        )
        .await;
        assert_eq!(
            (refused.status, refused.message()),
            (
                409,
                format!(
                    "Cannot update existing generic package version 'team-core/{package}/1.0.0' \
                     with status '{PACKAGE_VERSION_STATUS_PUBLISHED}'. Only \
                     '{PACKAGE_VERSION_STATUS_UNFINISHED}' package versions can be updated."
                )
            ),
            "case {case:?}: expected the closed version's refusal, got {refused:?}"
        );
    }
}
