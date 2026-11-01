// SPDX-License-Identifier: CC0-1.0

//! An app's rewrite and redirect rules ACROSS deployments: the rules are configured on
//! the APP and the files are published by the BRANCH, so a deployment that replaces the
//! files leaves the rules in place and they resolve onto whatever is published now.
//!
//! The sibling suite `hosted_branch_rewrites.rs` asserts what each rule DOES for one
//! published site; this one asserts the half neither of those cover — that the two halves
//! of a resolved request have independent lifetimes. Both drive the real wire path
//! (`MultiServiceRouter::dispatch`) through the same harness
//! ([`crate::hosted_branch_site::Hosted`]): the rules are installed with `UpdateApp`, the
//! files with a manual deployment, and the site is then visited with no credential at the
//! host the control plane reports for the branch.

use serde_json::json;

use crate::hosted_branch_site::{Hosted, SUCCEED};

/// A rewrite installed once resolves onto the files the LATEST successful deployment
/// published, and a deployment does not take the rules with it.
#[tokio::test]
async fn a_rule_resolves_onto_whatever_the_latest_deployment_published() {
    let hosted = Hosted::new("hosted-rules-redeploy", &json!({ "branchName": "main" })).await;

    // The rule the app carries for every case below: one source pattern, rewriting onto
    // the stylesheet the deployment publishes.
    let rules = json!([{ "source": "/assets/<*>", "target": "/styles/<*>", "status": "200" }]);
    let installed = hosted
        .emulator
        .update_app(&hosted.app_id, &json!({ "customRules": rules }))
        .await;
    assert_eq!(
        installed.status, 200,
        "expected HTTP 200 from UpdateApp, got {installed:?}"
    );

    // Each row: the case, the files the deployment carries, and the bytes the rewritten
    // path answers with afterwards — `None` when the deployment publishes no object the
    // rule's target names, which is a 404 rather than the previous deployment's bytes.
    type Row<'a> = (&'a str, Vec<(&'a str, &'a [u8])>, Option<&'a [u8]>);
    let rows: Vec<Row<'_>> = vec![
        (
            "the first deployment",
            vec![
                ("index.html", b"<html>one</html>" as &[u8]),
                ("styles/main.css", b"body{color:red}" as &[u8]),
            ],
            Some(b"body{color:red}" as &[u8]),
        ),
        (
            "a deployment that replaces the object the target names",
            vec![
                ("index.html", b"<html>two</html>" as &[u8]),
                ("styles/main.css", b"body{color:blue}" as &[u8]),
            ],
            Some(b"body{color:blue}" as &[u8]),
        ),
        (
            "a deployment that no longer publishes it",
            vec![("index.html", b"<html>three</html>" as &[u8])],
            None,
        ),
        (
            "a deployment that publishes it again",
            vec![
                ("index.html", b"<html>four</html>" as &[u8]),
                ("styles/main.css", b"body{color:green}" as &[u8]),
            ],
            Some(b"body{color:green}" as &[u8]),
        ),
    ];

    for (case, files, expected) in rows {
        let settled = hosted.deploy(case, &files).await;
        assert_eq!(
            settled, SUCCEED,
            "case {case:?}: the deployment the rule resolves against must succeed, got {settled}"
        );

        let visited = hosted.visit("/assets/main.css").await;
        match expected {
            Some(body) => {
                assert_eq!(
                    visited.status, 200,
                    "case {case:?}: the rule must still resolve the path, got {visited:?}"
                );
                assert_eq!(
                    visited.body, body,
                    "case {case:?}: expected the bytes this deployment published, {:?}, got {:?}",
                    String::from_utf8_lossy(body),
                    String::from_utf8_lossy(&visited.body)
                );
                assert_eq!(
                    visited.content_type.as_deref(),
                    Some("text/css"),
                    "case {case:?}: expected the resolved object's media type, got {visited:?}"
                );
            }
            None => assert_eq!(
                visited.status, 404,
                "case {case:?}: a rule whose target this deployment does not publish \
                 answers nothing, got {visited:?}"
            ),
        }

        // The rules are the APP's, so a deployment of the branch neither drops nor
        // rewrites them: the record a reader gets back still carries the list installed
        // once, above.
        let read_back = hosted.emulator.get_app(&hosted.app_id).await.member("app");
        assert_eq!(
            read_back.get("customRules"),
            Some(&rules),
            "case {case:?}: a deployment must leave the app's rules as they were, got {:?}",
            read_back.get("customRules")
        );
    }
}
