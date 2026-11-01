// SPDX-License-Identifier: CC0-1.0

//! The HOSTING EDGE of a deployed branch: the site a manual deployment published is
//! served at the default domain the control plane reports for that branch.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` — and reads the
//! host it visits out of the API itself (`GetApp`'s `defaultDomain` and `GetBranch`'s
//! `displayName`), exactly as a client that was told where its site is does.
//!
//! The rules under test, each over a TABLE spanning the class it generalises over
//! rather than over one recorded request:
//!
//! 1. A branch's site answers at `<displayName>.<defaultDomain>`: a `GET` of a path the
//!    deployment published answers HTTP 200, the bytes that were deployed at that path,
//!    and that file's own media type; the root — and any path naming a directory —
//!    answers the site's root document; a `HEAD` answers the same without the body.
//! 2. The edge serves that site and nothing else: a path the deployment did not publish
//!    is a 404, a method that is not a read is refused, and a host naming no branch of
//!    this emulator is not served at all.
//! 3. The site is the LATEST SUCCESSFUL deployment's: a later deployment that succeeds
//!    replaces it, and one that fails leaves the live site exactly as it was.
//! 4. A deployment's files go live when its JOB reaches its terminal status, so a branch
//!    nothing has deployed to serves nothing, and a visit straight after
//!    `StartDeployment` still sees the site as it was.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_amplify_amplify::state::JOB_SETTLE_MILLIS;

use crate::app_branch_lifecycle::{string_member, Emulator};

/// How long a case waits before expecting the clock-owed publication: the span the
/// service itself publishes for a job, plus a margin for a slow machine.
fn past_the_settle_span() -> Duration {
    Duration::from_millis(u64::try_from(JOB_SETTLE_MILLIS).unwrap_or(500) + 500)
}

/// One answer from the hosting edge, reduced to what a visitor reads.
///
/// `pub(crate)` for the same reason as [`Hosted`]: a sibling suite in this test target
/// asserting what the edge answers reads it through the same two types.
#[derive(Debug)]
pub(crate) struct Visited {
    pub(crate) status: u16,
    pub(crate) content_type: Option<String>,
    pub(crate) body: Vec<u8>,
    pub(crate) location: Option<String>,
}

/// A deployed branch: the emulator, the app and branch it was created on, and the host
/// its site answers at.
///
/// `pub(crate)` so a sibling suite — the one asserting how the app's rewrite and
/// redirect rules resolve a request — drives the same edge through the same harness
/// rather than standing a second one up beside it.
pub(crate) struct Hosted {
    pub(crate) emulator: Emulator,
    pub(crate) app_id: String,
    pub(crate) branch_name: String,
    pub(crate) host: String,
}

impl Hosted {
    /// An app with one branch, and the host the control plane reports that branch's site
    /// at: `Branch$displayName` "is used as the default domain prefix" and
    /// `App$defaultDomain` is "the default domain for the Amplify app"
    /// (`smithy_query(query="doc",shape="Branch",member="displayName")` /
    /// `(shape="App",member="defaultDomain")`), so the host is read out of those two
    /// members rather than assumed.
    pub(crate) async fn new(name: &str, branch_request: &Value) -> Self {
        let emulator = Emulator::new();
        let app_id = emulator.app_id_of_new_app(name).await;
        Self::adopt(emulator, app_id, branch_request).await
    }

    /// The same harness over an app that ALREADY exists — one created with its own
    /// configuration — so a suite asserting what that configuration does to the edge
    /// drives the one harness rather than a second copy of it.
    pub(crate) async fn adopt(emulator: Emulator, app_id: String, branch_request: &Value) -> Self {
        let created = emulator.create_branch(&app_id, branch_request).await;
        assert_eq!(
            created.status, 200,
            "app {app_id:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
        let branch_name = string_member(&created.member("branch"), "branchName");
        let display_name = string_member(&created.member("branch"), "displayName");
        let app = emulator.get_app(&app_id).await.member("app");
        let default_domain = string_member(&app, "defaultDomain");
        let host = format!("{display_name}.{default_domain}");
        Self {
            emulator,
            app_id,
            branch_name,
            host,
        }
    }

    /// Visit `path` on `host` exactly as a browser does: the method and the URL, and no
    /// credential of any kind.
    pub(crate) async fn visit_host(&self, method: &str, host: &str, path: &str) -> Visited {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .body(bytes::Bytes::new())
            .expect("a site URL is well formed");
        let response = self.emulator.dispatch(request).await;
        Visited {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            location: response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            body: response.into_body().to_vec(),
        }
    }

    /// Visit `path` on this branch's own host.
    pub(crate) async fn visit(&self, path: &str) -> Visited {
        self.visit_host("GET", &self.host, path).await
    }

    /// Deploy `files` as the zipped content of a reservation, and wait for the job the
    /// start call creates to settle. Answers the job's terminal status.
    pub(crate) async fn deploy(&self, case: &str, files: &[(&str, &[u8])]) -> String {
        let reserved = self
            .emulator
            .call(
                "POST",
                &format!(
                    "/apps/{}/branches/{}/deployments",
                    self.app_id, self.branch_name
                ),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            reserved.status, 200,
            "case {case:?}: expected HTTP 200 from CreateDeployment, got {reserved:?}"
        );
        let job_id = string_member(&reserved.body, "jobId");
        let url = string_member(&reserved.body, "zipUploadUrl");

        let upload = http::Request::builder()
            .method("PUT")
            .uri(&url)
            .body(bytes::Bytes::from(zip_of(files)))
            .expect("an upload destination is well formed");
        let status = self.emulator.dispatch(upload).await.status().as_u16();
        assert_eq!(
            status, 200,
            "case {case:?}: the destination {url:?} must accept the upload, got {status}"
        );

        let started = self
            .emulator
            .call(
                "POST",
                &format!(
                    "/apps/{}/branches/{}/deployments/start",
                    self.app_id, self.branch_name
                ),
                Some(&json!({ "jobId": job_id })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartDeployment, got {started:?}"
        );
        sleep(past_the_settle_span());
        let job = self.job(&job_id).await;
        string_member(&job.get("summary").cloned().unwrap_or(Value::Null), "status")
    }

    /// `GetJob` over the wire.
    async fn job(&self, job_id: &str) -> Value {
        self.emulator
            .call(
                "GET",
                &format!(
                    "/apps/{}/branches/{}/jobs/{job_id}",
                    self.app_id, self.branch_name
                ),
                None,
            )
            .await
            .member("job")
    }

    /// `StartDeployment` of a reservation whose zip is uploaded, WITHOUT waiting for the
    /// job to settle: what a visitor sees in the meantime is rule 4's business.
    async fn start_deploying(&self, case: &str, files: &[(&str, &[u8])]) -> String {
        let reserved = self
            .emulator
            .call(
                "POST",
                &format!(
                    "/apps/{}/branches/{}/deployments",
                    self.app_id, self.branch_name
                ),
                Some(&json!({})),
            )
            .await;
        let job_id = string_member(&reserved.body, "jobId");
        let url = string_member(&reserved.body, "zipUploadUrl");
        let upload = http::Request::builder()
            .method("PUT")
            .uri(&url)
            .body(bytes::Bytes::from(zip_of(files)))
            .expect("an upload destination is well formed");
        assert_eq!(
            self.emulator.dispatch(upload).await.status().as_u16(),
            200,
            "case {case:?}: the destination must accept the upload"
        );
        let started = self
            .emulator
            .call(
                "POST",
                &format!(
                    "/apps/{}/branches/{}/deployments/start",
                    self.app_id, self.branch_name
                ),
                Some(&json!({ "jobId": job_id })),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartDeployment, got {started:?}"
        );
        job_id
    }
}

/// A `.zip` carrying each named path with the content given, as a real archive a
/// deployment can be.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    for (path, content) in files {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("an archive entry starts");
        std::io::Write::write_all(&mut writer, content).expect("an archive entry is written");
    }
    writer.finish().expect("the archive closes");
    buffer.into_inner()
}

/// The terminal status of a deployment whose files were read and published.
pub(crate) const SUCCEED: &str = "SUCCEED";

/// The terminal status of a deployment whose files could not be published.
const FAILED: &str = "FAILED";

/// Rule 1: every path a deployment published is served, with its bytes and its own media
/// type, and the root answers the site's root document.
#[tokio::test]
async fn a_deployed_branch_serves_every_file_it_published() {
    let hosted = Hosted::new("hosted-site", &json!({ "branchName": "main" })).await;

    // The table spans the kinds of file a built web app deploys: the root document,
    // nested assets of several media types, and a file whose extension names no type at
    // all. Each row is (deployed path, content, the media type the edge answers).
    let rows: Vec<(&str, &[u8], &str)> = vec![
        ("index.html", b"<html>home</html>", "text/html"),
        ("assets/app.js", b"console.log(1)", "text/javascript"),
        ("assets/styles/main.css", b"body{color:red}", "text/css"),
        ("data/config.json", br#"{"a":1}"#, "application/json"),
        ("logo.svg", b"<svg/>", "image/svg+xml"),
        ("notes.txt", b"plain", "text/plain"),
        ("download.bin", b"\x00\x01\x02", "application/octet-stream"),
    ];
    let files: Vec<(&str, &[u8])> = rows.iter().map(|(path, body, _)| (*path, *body)).collect();
    let status = hosted.deploy("the whole site", &files).await;
    assert_eq!(
        status, SUCCEED,
        "case \"the whole site\": the deployment must succeed, got {status}"
    );

    for (path, content, media_type) in &rows {
        let visited = hosted.visit(&format!("/{path}")).await;
        assert_eq!(
            visited.status, 200,
            "case {path:?}: a published path must be served, got {visited:?}"
        );
        assert_eq!(
            visited.body, *content,
            "case {path:?}: the edge must serve the bytes that were deployed, got {:?}",
            String::from_utf8_lossy(&visited.body)
        );
        assert_eq!(
            visited.content_type.as_deref(),
            Some(*media_type),
            "case {path:?}: expected the media type {media_type:?}, got {:?}",
            visited.content_type
        );
    }

    // The root, and any path naming a directory, answer the site's root document.
    for path in ["/", "", "/assets/../"] {
        let visited = hosted.visit(path).await;
        if path == "/assets/../" {
            // A path that would climb out of the site names no published file.
            assert_eq!(
                visited.status, 404,
                "case {path:?}: a path leaving the site names no file, got {visited:?}"
            );
            continue;
        }
        assert_eq!(
            visited.status, 200,
            "case {path:?}: the root must serve the root document, got {visited:?}"
        );
        assert_eq!(
            visited.body, b"<html>home</html>",
            "case {path:?}: the root document's bytes must be served, got {:?}",
            String::from_utf8_lossy(&visited.body)
        );
    }

    // A HEAD is the same answer without the body.
    let head = hosted.visit_host("HEAD", &hosted.host, "/index.html").await;
    assert_eq!(
        head.status, 200,
        "case \"HEAD /index.html\": expected HTTP 200, got {head:?}"
    );
    assert!(
        head.body.is_empty(),
        "case \"HEAD /index.html\": a HEAD carries no body, got {head:?}"
    );
    assert_eq!(
        head.content_type.as_deref(),
        Some("text/html"),
        "case \"HEAD /index.html\": the type is the file's, got {head:?}"
    );
}

/// Rule 2: the edge serves that site and nothing else.
#[tokio::test]
async fn the_edge_serves_only_what_was_deployed() {
    let hosted = Hosted::new("hosted-scope", &json!({ "branchName": "main" })).await;
    let status = hosted
        .deploy(
            "one file",
            &[("index.html", b"<html>only</html>" as &[u8])],
        )
        .await;
    assert_eq!(status, SUCCEED, "the deployment must succeed, got {status}");

    // Paths the deployment did not publish, spanning the ways one can be asked for.
    for path in [
        "/missing.html",
        "/assets/app.js",
        "/index.html/more",
        "/deep/nested/file.css",
    ] {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, 404,
            "case {path:?}: an unpublished path must be a 404, got {visited:?}"
        );
    }

    // A hosting edge serves a site; it takes no writes.
    for method in ["PUT", "POST", "DELETE", "PATCH"] {
        let visited = hosted.visit_host(method, &hosted.host, "/index.html").await;
        assert_eq!(
            visited.status, 405,
            "case {method:?}: the edge is read-only, got {visited:?}"
        );
    }

    // A host that names no branch of this emulator is served by nobody: the published
    // site of one branch is not reachable under another name.
    for host in [
        "another-branch.d000000000000.amplifyapp.com",
        "main.example.invalid",
    ] {
        let visited = hosted.visit_host("GET", host, "/index.html").await;
        assert_eq!(
            visited.status, 404,
            "case {host:?}: a host naming no branch is not served, got {visited:?}"
        );
    }
}

/// Rule 3: the live site is the latest SUCCESSFUL deployment's.
#[tokio::test]
async fn a_later_deployment_replaces_the_site_only_when_it_succeeds() {
    let hosted = Hosted::new("hosted-replacement", &json!({ "branchName": "main" })).await;

    // Each row: the case, the files the deployment carries, the status its job settles
    // to, and what the site serves at the root afterwards.
    type Row<'a> = (&'a str, Vec<(&'a str, &'a [u8])>, &'a str, &'a [u8]);
    let rows: Vec<Row<'_>> = vec![
        (
            "the first deployment",
            vec![("index.html", b"<html>one</html>" as &[u8])],
            SUCCEED,
            b"<html>one</html>",
        ),
        (
            "a second deployment that succeeds",
            vec![
                ("index.html", b"<html>two</html>" as &[u8]),
                ("app.js", b"two()" as &[u8]),
            ],
            SUCCEED,
            b"<html>two</html>",
        ),
        (
            "a deployment carrying no root document",
            vec![("site/index.html", b"<html>three</html>" as &[u8])],
            FAILED,
            b"<html>two</html>",
        ),
        (
            "a third deployment that succeeds",
            vec![("index.html", b"<html>four</html>" as &[u8])],
            SUCCEED,
            b"<html>four</html>",
        ),
    ];

    for (case, files, status, served) in rows {
        let settled = hosted.deploy(case, &files).await;
        assert_eq!(
            settled, status,
            "case {case:?}: expected the job to settle to {status}, got {settled}"
        );
        let visited = hosted.visit("/").await;
        assert_eq!(
            visited.status, 200,
            "case {case:?}: the site must keep serving, got {visited:?}"
        );
        assert_eq!(
            visited.body, served,
            "case {case:?}: expected the root to serve {:?}, got {:?}",
            String::from_utf8_lossy(served),
            String::from_utf8_lossy(&visited.body)
        );
    }

    // The file only the replaced deployment carried is gone with it.
    let visited = hosted.visit("/app.js").await;
    assert_eq!(
        visited.status, 404,
        "case \"a replaced file\": the live site is one deployment's, got {visited:?}"
    );
}

/// Rule 4: nothing is served before a deployment's job has settled.
#[tokio::test]
async fn a_site_goes_live_when_its_deployment_job_settles() {
    // The branch carries its OWN display name, which is the prefix of the host its site
    // answers at — so this case also spans a branch whose display name is not its name.
    let hosted = Hosted::new(
        "hosted-timing",
        &json!({ "branchName": "release-1-0", "displayName": "release-one" }),
    )
    .await;
    assert!(
        hosted.host.starts_with("release-one."),
        "the host is built from the display name, got {:?}",
        hosted.host
    );

    // A branch nothing has been deployed to serves nothing, at any path.
    for path in ["/", "/index.html"] {
        let visited = hosted.visit(path).await;
        assert_eq!(
            visited.status, 404,
            "case {path:?}: a branch with no deployment serves nothing, got {visited:?}"
        );
    }

    // Straight after `StartDeployment` the job is still PENDING, so the files are not
    // live yet: the span is non-zero.
    let job_id = hosted
        .start_deploying(
            "the first deployment",
            &[("index.html", b"<html>live</html>" as &[u8])],
        )
        .await;
    let during = hosted.visit("/").await;
    assert_eq!(
        during.status, 404,
        "case \"during the deployment\": the files go live when the job settles, got {during:?}"
    );

    sleep(past_the_settle_span());
    let settled = hosted.job(&job_id).await;
    assert_eq!(
        string_member(
            &settled.get("summary").cloned().unwrap_or(Value::Null),
            "status"
        ),
        SUCCEED,
        "case \"after the span\": the job must have settled, got {settled:?}"
    );
    let after = hosted.visit("/").await;
    assert_eq!(
        after.status, 200,
        "case \"after the span\": the site must be live, got {after:?}"
    );
    assert_eq!(
        after.body, b"<html>live</html>",
        "case \"after the span\": the deployed root document must be served, got {:?}",
        String::from_utf8_lossy(&after.body)
    );

    // Visiting the site changes nothing a later visitor sees.
    let again = hosted.visit("/").await;
    assert_eq!(
        (again.status, &again.body),
        (after.status, &after.body),
        "case \"a second visit\": a read must not change the site, got {again:?}"
    );
}
