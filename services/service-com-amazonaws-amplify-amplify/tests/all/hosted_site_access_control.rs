// SPDX-License-Identifier: CC0-1.0

//! The ACCESS CONTROL of an Amplify branch's site: the basic authorization an app
//! configures for its branches, enforced at the hosting edge, and what the API reports
//! about the credentials it was configured with.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` — through the
//! same harness the rest of the edge suites use
//! ([`crate::hosted_branch_site::Hosted`]): the credentials are configured with
//! `CreateApp`/`CreateBranch`/`UpdateBranch`, the site is published by a manual
//! deployment, and the site is then visited at the host the control plane reports for the
//! branch, with and without an `Authorization` header.
//!
//! The two rules under test, each over a table spanning the class it generalises over
//! rather than the one recorded request:
//!
//! 1. **A site whose basic authorization is enabled is served only to a request that
//!    presents the configured credentials.** `Branch$enableBasicAuth` enables it for the
//!    branch and `App$enableBasicAuth` "enables basic authorization for the Amplify app's
//!    branches"; the credentials verified are the branch's own
//!    (`Branch$basicAuthCredentials`, base64 of `user:password`) and otherwise the app's
//!    ("the basic authorization credentials for branches for the Amplify app"). Every
//!    other request — none presented, the wrong password, the wrong user, another scheme
//!    — is answered `401` with a `WWW-Authenticate: Basic` challenge and nothing about
//!    the site, whether or not the path it asked for exists. Turning the flag off serves
//!    the site openly again.
//! 2. **A configured credential is never reported back.** `BasicAuthCredentials` is
//!    `@sensitive` (`smithy_query(query="shape",shape="BasicAuthCredentials")`): every
//!    operation that answers with the record reports a reference to the credential that
//!    is not the submitted value and does not carry the password, is the same across
//!    reads, and differs for a different credential.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator};
use crate::hosted_branch_site::{Hosted, Visited, SUCCEED};

/// The credentials member as the model documents it: "base64-encode the authorization
/// credentials and provide them in the format `user:password`".
fn configured(user: &str, password: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{user}:{password}"))
}

/// The bytes the page every case visits was deployed with.
const PAGE: &[u8] = b"<html>protected</html>";

/// The file the deployment publishes, and a path it does not.
const PUBLISHED: &str = "/index.html";
const UNPUBLISHED: &str = "/secret/plan.txt";

/// Assert that `visited` is the challenge a protected site answers: the `401`, a `Basic`
/// challenge asking for the credentials, and none of the site's bytes.
fn assert_challenged(case: &str, path: &str, visited: &Visited) {
    assert_eq!(
        visited.status, 401,
        "case {case:?}: {path} must answer 401 while the site is protected, got {visited:?}"
    );
    let challenge = visited.header("www-authenticate").unwrap_or_default();
    assert!(
        challenge.starts_with("Basic"),
        "case {case:?}: {path} must carry a Basic challenge, got {challenge:?}"
    );
    assert!(
        !visited.body.windows(PAGE.len()).any(|window| window == PAGE),
        "case {case:?}: {path} must disclose none of the site, got {:?}",
        String::from_utf8_lossy(&visited.body)
    );
}

/// Assert that `visited` is the page itself.
fn assert_served(case: &str, visited: &Visited) {
    assert_eq!(
        visited.status, 200,
        "case {case:?}: the configured credentials must be served the site, got {visited:?}"
    );
    assert_eq!(
        visited.body, PAGE,
        "case {case:?}: expected the deployed bytes, got {:?}",
        String::from_utf8_lossy(&visited.body)
    );
}

/// Visit the published page with the `Authorization` header `authorization`, or with none
/// at all.
async fn visit_as(hosted: &Hosted, path: &str, authorization: Option<&str>) -> Visited {
    match authorization {
        Some(value) => {
            hosted
                .visit_with("GET", &hosted.host, path, &[("authorization", value)])
                .await
        }
        None => hosted.visit_host("GET", &hosted.host, path).await,
    }
}

/// Rule 1, on a branch that configures its own credentials.
#[tokio::test]
async fn a_protected_branch_serves_its_site_only_to_the_configured_credentials() {
    let hosted = Hosted::new("hosted-access", &json!({ "branchName": "main" })).await;
    let settled = hosted
        .deploy("the published page", &[("index.html", PAGE)])
        .await;
    assert_eq!(settled, SUCCEED, "the deployment must succeed, got {settled}");

    // Before any access control is configured the page is public, and nothing challenges
    // for credentials — so every 401 below is this configuration's doing.
    let open = visit_as(&hosted, PUBLISHED, None).await;
    assert_served("no access control configured", &open);
    assert!(
        open.header("www-authenticate").is_none(),
        "case \"no access control configured\": an open site challenges nobody, got {open:?}"
    );

    let credentials = configured("probe", "s3cret-probe-01");
    let turned_on = hosted
        .emulator
        .update_branch(
            &hosted.app_id,
            &hosted.branch_name,
            &json!({ "enableBasicAuth": true, "basicAuthCredentials": credentials }),
        )
        .await;
    assert_eq!(
        turned_on.status, 200,
        "expected HTTP 200 from UpdateBranch, got {turned_on:?}"
    );

    // Each row: the case, the `Authorization` header the visitor presents, and whether
    // the site is served to it. The rows span the ways a credential can be presented or
    // missing, not only the recorded "none" and "wrong password".
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no Authorization header at all", None, false),
        (
            "the configured credentials",
            Some(format!("Basic {credentials}")),
            true,
        ),
        (
            "the scheme written in lower case",
            Some(format!("basic {credentials}")),
            true,
        ),
        (
            "the wrong password",
            Some(format!("Basic {}", configured("probe", "wrong"))),
            false,
        ),
        (
            "the right password under another user",
            Some(format!("Basic {}", configured("other", "s3cret-probe-01"))),
            false,
        ),
        (
            "the password with no user part",
            Some(format!("Basic {}", ::aws_smithy_types::base64::encode("s3cret-probe-01"))),
            false,
        ),
        (
            "a token that is not base64",
            Some("Basic %%%not-base64%%%".to_string()),
            false,
        ),
        (
            "another authentication scheme",
            Some(format!("Bearer {credentials}")),
            false,
        ),
        ("an empty Authorization header", Some(String::new()), false),
    ];
    for (case, authorization, served) in rows {
        let visited = visit_as(&hosted, PUBLISHED, authorization.as_deref()).await;
        if served {
            assert_served(case, &visited);
        } else {
            assert_challenged(case, PUBLISHED, &visited);
            // A protected site says nothing about what it carries: a path it does not
            // publish is the same challenge, not a 404 that would confirm the rest.
            let probed = visit_as(&hosted, UNPUBLISHED, authorization.as_deref()).await;
            assert_challenged(case, UNPUBLISHED, &probed);
        }
    }

    // The flag decides, in both directions: turned off, the page is public again; turned
    // back on WITHOUT re-sending the credentials, the credentials configured once still
    // decide.
    for (case, request, authorized_without_credentials) in [
        ("access control turned off", json!({ "enableBasicAuth": false }), true),
        ("access control turned on again", json!({ "enableBasicAuth": true }), false),
    ] {
        let updated = hosted
            .emulator
            .update_branch(&hosted.app_id, &hosted.branch_name, &request)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateBranch, got {updated:?}"
        );
        let anonymous = visit_as(&hosted, PUBLISHED, None).await;
        if authorized_without_credentials {
            assert_served(case, &anonymous);
        } else {
            assert_challenged(case, PUBLISHED, &anonymous);
            let with_credentials =
                visit_as(&hosted, PUBLISHED, Some(&format!("Basic {credentials}"))).await;
            assert_served(case, &with_credentials);
        }
    }
}

/// Rule 1, on the credentials the APP configures for its branches: they protect a branch
/// that configures none of its own, and a branch that does configure one is verified
/// against its own.
#[tokio::test]
async fn an_apps_access_control_protects_a_branch_that_configures_none() {
    let app_credentials = configured("app-probe", "app-password-02");
    let emulator = Emulator::new();
    let created = emulator
        .create_app(&json!({
            "name": "hosted-access-app-wide",
            "enableBasicAuth": true,
            "basicAuthCredentials": app_credentials,
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateApp, got {created:?}"
    );
    let app_id = string_member(&created.member("app"), "appId");
    let hosted = Hosted::adopt(emulator, app_id, &json!({ "branchName": "main" })).await;
    let settled = hosted
        .deploy("the published page", &[("index.html", PAGE)])
        .await;
    assert_eq!(settled, SUCCEED, "the deployment must succeed, got {settled}");

    // The branch itself enables nothing, so this protection is the app's.
    assert_challenged(
        "the app's access control, no credentials presented",
        PUBLISHED,
        &visit_as(&hosted, PUBLISHED, None).await,
    );
    assert_served(
        "the app's credentials",
        &visit_as(&hosted, PUBLISHED, Some(&format!("Basic {app_credentials}"))).await,
    );

    // A branch that configures its OWN credentials is verified against those: the app's
    // no longer open its site.
    let branch_credentials = configured("branch-probe", "branch-password-03");
    let updated = hosted
        .emulator
        .update_branch(
            &hosted.app_id,
            &hosted.branch_name,
            &json!({ "basicAuthCredentials": branch_credentials }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200 from UpdateBranch, got {updated:?}"
    );
    assert_served(
        "the branch's own credentials",
        &visit_as(&hosted, PUBLISHED, Some(&format!("Basic {branch_credentials}"))).await,
    );
    assert_challenged(
        "the app's credentials once the branch configures its own",
        PUBLISHED,
        &visit_as(&hosted, PUBLISHED, Some(&format!("Basic {app_credentials}"))).await,
    );
}

/// Rule 1, where basic authorization is enabled and NO credentials are configured
/// anywhere: there is nothing a visitor could present, so the site is served to nobody
/// rather than to everybody.
#[tokio::test]
async fn a_site_protected_with_no_credentials_is_served_to_nobody() {
    let hosted = Hosted::new(
        "hosted-access-no-credentials",
        &json!({ "branchName": "main", "enableBasicAuth": true }),
    )
    .await;
    let settled = hosted
        .deploy("the published page", &[("index.html", PAGE)])
        .await;
    assert_eq!(settled, SUCCEED, "the deployment must succeed, got {settled}");

    for (case, authorization) in [
        ("no credentials presented", None),
        (
            "a guess at the credentials",
            Some(format!("Basic {}", configured("probe", "guess"))),
        ),
        (
            "the empty credential pair",
            Some(format!("Basic {}", configured("", ""))),
        ),
    ] {
        assert_challenged(
            case,
            PUBLISHED,
            &visit_as(&hosted, PUBLISHED, authorization.as_deref()).await,
        );
    }
}

/// Rule 2, over every operation that answers with a branch or an app record.
#[tokio::test]
async fn a_configured_credential_is_reported_as_a_reference_and_never_echoed() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("credential-reporting").await;

    // The rows span the credential's shape: the documented pair, a long password, a
    // password carrying the separator, one carrying non-alphanumeric bytes, and the
    // credential at a length no recording cites.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the documented user:password pair",
            configured("user", "password"),
            "password".to_string(),
        ),
        (
            "a long password",
            configured("probe", &"p".repeat(200)),
            "p".repeat(200),
        ),
        (
            "a password carrying the separator",
            configured("probe", "a:b:c"),
            "a:b:c".to_string(),
        ),
        (
            "a password of punctuation",
            configured("probe", "#?&=+%$-_"),
            "#?&=+%$-_".to_string(),
        ),
        (
            "a user and password of one character each",
            configured("u", "p"),
            "u:p".to_string(),
        ),
    ];

    let mut references = Vec::new();
    for (index, (case, credentials, password)) in rows.iter().enumerate() {
        // The same credential configured at CREATE time and by an UPDATE: both ingest it
        // through the same rule, so both report the same reference.
        let created = emulator
            .create_branch(
                &app_id,
                &json!({
                    "branchName": format!("created-{index}"),
                    "enableBasicAuth": true,
                    "basicAuthCredentials": credentials,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        let updated_name = format!("updated-{index}");
        let plain = emulator
            .create_branch(&app_id, &json!({ "branchName": updated_name }))
            .await;
        assert_eq!(
            plain.status, 200,
            "case {case:?}: expected HTTP 200 from CreateBranch, got {plain:?}"
        );
        assert_eq!(
            plain.member("branch").get("basicAuthCredentials"),
            Some(&Value::Null),
            "case {case:?}: a branch that configured no credential reports none, got {:?}",
            plain.member("branch").get("basicAuthCredentials")
        );
        let updated = emulator
            .update_branch(
                &app_id,
                &updated_name,
                &json!({ "enableBasicAuth": true, "basicAuthCredentials": credentials }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateBranch, got {updated:?}"
        );

        // Every answer about either branch reports the same reference: the two writers,
        // the single read, and the list element.
        let read_created = emulator
            .get_branch(&app_id, &format!("created-{index}"))
            .await
            .member("branch");
        let read_updated = emulator
            .get_branch(&app_id, &updated_name)
            .await
            .member("branch");
        let listed = emulator.list_branches(&app_id, "").await.member("branches");
        let from_list: Vec<Value> = listed
            .as_array()
            .expect("branches is a list")
            .iter()
            .filter(|branch| {
                let name = string_member(branch, "branchName");
                name == format!("created-{index}") || name == updated_name
            })
            .map(|branch| {
                branch
                    .get("basicAuthCredentials")
                    .cloned()
                    .unwrap_or(Value::Null)
            })
            .collect();

        let reference = string_member(&read_created, "basicAuthCredentials");
        let reported = [
            ("CreateBranch", string_member(&created.member("branch"), "basicAuthCredentials")),
            ("UpdateBranch", string_member(&updated.member("branch"), "basicAuthCredentials")),
            ("GetBranch, created", reference.clone()),
            ("GetBranch, updated", string_member(&read_updated, "basicAuthCredentials")),
            ("ListBranches, created", string_member_of(&from_list, 0)),
            ("ListBranches, updated", string_member_of(&from_list, 1)),
        ];
        for (operation, value) in &reported {
            assert_eq!(
                value, &reference,
                "case {case:?}: {operation} must report the same reference as every other \
                 answer, got {value:?} against {reference:?}"
            );
            assert!(
                !value.is_empty() && value != credentials,
                "case {case:?}: {operation} must report neither nothing nor the submitted \
                 credential, got {value:?}"
            );
            let decoded = ::aws_smithy_types::base64::decode(value)
                .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
                .unwrap_or_default();
            assert!(
                !decoded.contains(password.as_str()),
                "case {case:?}: {operation}'s reported value must not decode to the \
                 configured password, got {decoded:?}"
            );
        }

        // A read changes nothing: the reference is the same the next time it is asked
        // for.
        let again = emulator
            .get_branch(&app_id, &format!("created-{index}"))
            .await
            .member("branch");
        assert_eq!(
            string_member(&again, "basicAuthCredentials"),
            reference,
            "case {case:?}: the reported reference must be stable across reads"
        );
        references.push((case, reference));
    }

    // A different credential is a different reference, so a client comparing two
    // configurations still sees that they differ.
    for (case, reference) in &references {
        let collisions = references
            .iter()
            .filter(|(_, other)| other == reference)
            .count();
        assert_eq!(
            collisions, 1,
            "case {case:?}: a credential's reference must differ from every other \
             credential's, got {references:?}"
        );
    }

    // The same rule on the APP's own credentials member, which carries the credentials
    // for its branches.
    let credentials = configured("app-probe", "app-password-04");
    let created = emulator
        .create_app(&json!({
            "name": "credential-reporting-app",
            "enableBasicAuth": true,
            "basicAuthCredentials": credentials,
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "expected HTTP 200 from CreateApp, got {created:?}"
    );
    let created_app = string_member(&created.member("app"), "appId");
    let reference = string_member(
        &emulator.get_app(&created_app).await.member("app"),
        "basicAuthCredentials",
    );
    assert_eq!(
        string_member(&created.member("app"), "basicAuthCredentials"),
        reference,
        "case \"CreateApp\": the create answer and GetApp must report the same reference"
    );
    assert!(
        !reference.is_empty() && reference != credentials,
        "case \"CreateApp\": the app's credential must not be echoed, got {reference:?}"
    );

    let updated = emulator
        .update_app(&app_id, &json!({ "basicAuthCredentials": credentials }))
        .await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200 from UpdateApp, got {updated:?}"
    );
    assert_eq!(
        string_member(&updated.member("app"), "basicAuthCredentials"),
        reference,
        "case \"UpdateApp\": the same credential must report the same reference whichever \
         app and whichever writer configured it"
    );

    // The same member inside the auto-branch-creation configuration — the credential a
    // branch Amplify creates automatically is given — is reported the same way, by both
    // writers of that configuration.
    let configured_in = emulator
        .update_app(
            &app_id,
            &json!({
                "enableAutoBranchCreation": true,
                "autoBranchCreationConfig": {
                    "stage": "EXPERIMENTAL",
                    "enableBasicAuth": true,
                    "basicAuthCredentials": credentials,
                },
            }),
        )
        .await;
    assert_eq!(
        configured_in.status, 200,
        "expected HTTP 200 from UpdateApp, got {configured_in:?}"
    );
    for (operation, record) in [
        ("UpdateApp", configured_in.member("app")),
        ("GetApp", emulator.get_app(&app_id).await.member("app")),
    ] {
        let config = record
            .get("autoBranchCreationConfig")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string_member(&config, "basicAuthCredentials"),
            reference,
            "case {operation:?}: the configuration's credential must be reported as the \
             same reference, got {config:?}"
        );
    }
}

/// The `index`th value of a list of already-extracted members, as a string.
fn string_member_of(values: &[Value], index: usize) -> String {
    values
        .get(index)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("value {index} is not a string in {values:?}"))
        .to_string()
}
