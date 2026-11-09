// SPDX-License-Identifier: CC0-1.0

//! The TEXT CodeCommit puts on `FileDoesNotExistException`, over the wire.
//!
//! The rule under test: the message a refusal carries is the one the OPERATION that
//! raised it answers that error class with — no operation borrows another's
//! template. For a path a tree does not hold, the reading operations (`GetFile`, and
//! `CreateCommit` naming a path through each of its three entry kinds) answer ONE
//! template — the fixed words "Could not find path " followed by that path — and
//! nothing else of the request or of the stored state reaches it: not the commit the
//! path was resolved against, not the branch, not the repository. It is the path
//! sibling of the `CommitDoesNotExistException` text (for a commit specifier that
//! resolved to nothing, "AWS CodeCommit could not find the commit specifier
//! <specifier>. …") the neighbouring suites pin. `DeleteFile` answers the same code
//! with its OWN fixed sentence, which names no path at all.
//!
//! So the tables below vary what the rule generalises over — where the path sits
//! (repository root, one folder, four folders deep), how it is spelled (bare, with a
//! leading separator, with a space, dotted, non-ASCII, without an extension), WHICH
//! operation raises the code, and, for the read, which commit specifier form
//! resolved the tree (absent, a branch name, an explicit commit id) — and assert the
//! template that operation words the refusal with on every row, plus that no row's
//! text carries the repository, the branch or any commit id.
//!
//! The error's status and code come from the model
//! (`smithy_query(query="shape",shape="FileDoesNotExistException")`: `errorKind`
//! client, `httpError` 400); a path with a leading separator addresses the same tree
//! entry as one without it, so the read template's slot carries the path in that
//! canonical form.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness — building the emulator, signing a call, reading an answer — is
// the shared one in `crate::repository_lifecycle`, and the reader that answers a
// refusal's code AND its message is `crate::harness`'s, so no two suites can
// disagree about what calling the service or reading its refusal means.
use crate::harness::error_with_message;
use crate::repository_lifecycle::{base64, emulator, ok, string_at};

/// The HTTP status the model gives `FileDoesNotExistException` (`errorKind: client`,
/// `httpError: 400`).
const CLIENT_ERROR_STATUS: u16 = 400;

/// The code every row asserts.
const CODE: &str = "FileDoesNotExistException";

/// The branch every case's repository holds.
const BRANCH: &str = "main";

/// The message the rule requires for a path on the READ paths: the fixed words, then
/// the path.
fn expected_message(addressed_path: &str) -> String {
    format!("Could not find path {addressed_path}")
}

/// The message `DeleteFile` answers the same code with: FIXED text that names no
/// path, which is the whole of that operation's template.
const DELETE_MESSAGE: &str = "Cannot find the specified file.";

/// One row: a path no repository under test holds, as SENT and as it ADDRESSES a
/// tree entry (leading separators removed — `"/a.txt"` and `"a.txt"` name the same
/// entry).
#[derive(Debug, Clone, Copy)]
struct Absent {
    label: &'static str,
    sent: &'static str,
    addressed: &'static str,
}

/// The table. Rows differ along where the path sits, how it is spelled and how long
/// it is — none of them the path the recorded case used.
fn absent_paths() -> Vec<Absent> {
    vec![
        Absent {
            label: "a path at the root of the tree",
            sent: "absent.txt",
            addressed: "absent.txt",
        },
        Absent {
            label: "a path in one folder",
            sent: "docs/missing.md",
            addressed: "docs/missing.md",
        },
        Absent {
            label: "a path four folders deep",
            sent: "a/b/c/d/deep.txt",
            addressed: "a/b/c/d/deep.txt",
        },
        Absent {
            label: "a path sent with a leading separator",
            sent: "/leading-separator.txt",
            addressed: "leading-separator.txt",
        },
        Absent {
            label: "a non-ASCII path",
            sent: "año/informe-anual.txt",
            addressed: "año/informe-anual.txt",
        },
        Absent {
            label: "a path with no extension",
            sent: "scripts/bootstrap",
            addressed: "scripts/bootstrap",
        },
        Absent {
            label: "a path with a space in it",
            sent: "release notes.txt",
            addressed: "release notes.txt",
        },
        Absent {
            label: "a dotted path",
            sent: "dist/bundle.v2.tar.gz",
            addressed: "dist/bundle.v2.tar.gz",
        },
    ]
}

/// A repository holding two commits on [`BRANCH`]: the first a single file, the
/// second adding one in a folder. Answers `(first commit id, tip commit id)`, so a
/// read can be resolved against either.
async fn seed(router: &MultiServiceRouter, repository: &str) -> (String, String) {
    ok(
        router,
        "CreateRepository",
        &json!({ "repositoryName": repository }),
    )
    .await;
    let first = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": BRANCH,
            "filePath": "README.md",
            "fileContent": base64(b"# demo\n"),
        }),
    )
    .await;
    let first = string_at(&first, "/commitId");
    let second = ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": BRANCH,
            "filePath": "src/lib.rs",
            "fileContent": base64(b"pub fn one() {}\n"),
            "parentCommitId": first.clone(),
        }),
    )
    .await;
    let tip = string_at(&second, "/commitId");
    (first, tip)
}

/// Assert one refusal against the rule: the modelled code, the exact template, and
/// nothing of the request or the state beyond the path in the text.
fn assert_refusal(case: &str, code: &str, message: &str, path: &str, must_not_name: &[&str]) {
    assert_eq!(
        code, CODE,
        "case {case:?}: expected the modelled {CODE}, got {code:?} with message {message:?}"
    );
    assert_eq!(
        message,
        expected_message(path),
        "case {case:?}: expected the exact message AWS words for an absent path, got {message:?}"
    );
    for named in must_not_name {
        assert!(
            !message.contains(named),
            "case {case:?}: the message must name only the path, but {message:?} carries {named:?}"
        );
    }
}

#[tokio::test]
async fn get_file_words_an_absent_path_with_the_path_template() {
    let router = emulator();
    let repository = "absent-path-read";
    let (first, tip) = seed(&router, repository).await;

    // Which commit the tree came from does not reach the text, so every specifier
    // form — absent (the default branch's HEAD), a branch name, either commit id —
    // must answer the same message.
    let specifiers: Vec<(&str, Option<String>)> = vec![
        ("no commitSpecifier", None),
        ("a branch name", Some(BRANCH.to_string())),
        ("the tip commit id", Some(tip.clone())),
        ("an earlier commit id", Some(first.clone())),
    ];

    for absent in absent_paths() {
        for (specifier_label, specifier) in &specifiers {
            let case = format!("{} read with {specifier_label}", absent.label);
            let mut request = json!({
                "repositoryName": repository,
                "filePath": absent.sent,
            });
            if let Some(specifier) = specifier {
                request["commitSpecifier"] = json!(specifier);
            }
            let (code, message) =
                error_with_message(&router, "GetFile", &request, CLIENT_ERROR_STATUS).await;
            assert_refusal(
                &case,
                &code,
                &message,
                absent.addressed,
                &[repository, BRANCH, first.as_str(), tip.as_str()],
            );
        }
    }

    // The positive control: a path the tree DOES hold is served, so the rows above
    // are refusals about the path and not about the repository or the specifier.
    let got = ok(
        &router,
        "GetFile",
        &json!({ "repositoryName": repository, "filePath": "src/lib.rs" }),
    )
    .await;
    assert_eq!(
        string_at(&got, "/filePath"),
        "src/lib.rs",
        "a path the tree holds must still be served, got {got}"
    );
}

#[tokio::test]
async fn delete_file_words_an_absent_path_with_its_own_fixed_sentence() {
    let router = emulator();
    let repository = "absent-path-delete";
    let (first, tip) = seed(&router, repository).await;

    for absent in absent_paths() {
        let (code, message) = error_with_message(
            &router,
            "DeleteFile",
            &json!({
                "repositoryName": repository,
                "branchName": BRANCH,
                "filePath": absent.sent,
                "parentCommitId": tip.clone(),
            }),
            CLIENT_ERROR_STATUS,
        )
        .await;
        assert_eq!(
            code, CODE,
            "case {:?}: expected the modelled {CODE}, got {code:?} with message {message:?}",
            absent.label
        );
        assert_eq!(
            message, DELETE_MESSAGE,
            "case {:?}: expected the exact sentence AWS words a deletion's absent path with, \
             got {message:?}",
            absent.label
        );
        // The delete path's sentence is opaque: it names neither the path the request
        // carried nor anything else about the repository it was made against.
        for named in [absent.sent, absent.addressed, repository, BRANCH, &first, &tip] {
            assert!(
                !message.contains(named),
                "case {:?}: the message must name nothing of the request, but {message:?} \
                 carries {named:?}",
                absent.label
            );
        }
    }

    // A refused deletion writes nothing, so the branch still points at the tip the
    // requests named.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": BRANCH }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "a refused deletion must leave the branch where it was, got {branch}"
    );
}

#[tokio::test]
async fn create_commit_words_an_absent_path_with_the_path_template() {
    let router = emulator();
    let repository = "absent-path-commit";
    let (first, tip) = seed(&router, repository).await;

    // The three entry kinds that can name a path the tree does not hold: a deletion,
    // a mode change, and the `sourceFile` a put copies from.
    type Entry = (&'static str, fn(&str) -> Value);
    let entries: Vec<Entry> = vec![
        ("a deleteFiles entry", |path| {
            json!({ "deleteFiles": [{ "filePath": path }] })
        }),
        ("a setFileModes entry", |path| {
            json!({ "setFileModes": [{ "filePath": path, "fileMode": "EXECUTABLE" }] })
        }),
        ("a putFiles sourceFile", |path| {
            json!({
                "putFiles": [{
                    "filePath": "copied.txt",
                    "sourceFile": { "filePath": path },
                }],
            })
        }),
    ];

    for absent in absent_paths() {
        for (entry_label, build) in &entries {
            let case = format!("{} named by {entry_label}", absent.label);
            let mut request = json!({
                "repositoryName": repository,
                "branchName": BRANCH,
                "parentCommitId": tip.clone(),
                "commitMessage": "a change naming a path the tree does not hold",
            });
            let Value::Object(change) = build(absent.sent) else {
                panic!("case {case:?}: the table builds an object");
            };
            for (member, value) in change {
                request[member] = value;
            }
            let (code, message) =
                error_with_message(&router, "CreateCommit", &request, CLIENT_ERROR_STATUS).await;
            assert_refusal(
                &case,
                &code,
                &message,
                absent.addressed,
                &[repository, BRANCH, first.as_str(), tip.as_str()],
            );
        }
    }

    // Nothing was committed by any refused request.
    let branch = ok(
        &router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": BRANCH }),
    )
    .await;
    assert_eq!(
        string_at(&branch, "/branch/commitId"),
        tip,
        "a refused commit must leave the branch where it was, got {branch}"
    );
}
