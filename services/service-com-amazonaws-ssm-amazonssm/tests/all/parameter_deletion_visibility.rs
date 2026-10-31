// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteParameter` takes away, read back through every OTHER Parameter Store
//! operation this crate implements.
//!
//! The rule under test: **a delete removes the parameter its `Name` resolves to WHOLE
//! from the one store the slice shares — every retained version of it and its tags — so
//! no operation that reports parameters reports that name afterwards, the parameters it
//! did not address keep being reported, and a later write of the same name is a fresh
//! parameter starting at version 1 rather than a continuation of the deleted one.**
//!
//! `all/parameter_deletion.rs` pins the delete against `GetParameter` (including the
//! version-selector forms and the repeated delete). This suite pins the same removal
//! against the rest of the slice's readers — `GetParameterHistory`, `DescribeParameters`
//! (by name AND by tag) and `GetParametersByPath` — because a delete that only hid the
//! parameter from one reader would leave the store holding it.
//!
//! The table's rows differ along what the rule generalises over: the name's form (plain,
//! and hierarchies of two different depths), how many versions the parameter carries,
//! its type, and how many tags it has (none, one, two). The front-door harness is this
//! crate's one harness (`all/parameter_store.rs`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, router};

/// One row: a parameter to write, and the path read that reports it.
struct Row {
    case: &'static str,
    /// The name the parameter is written under and addressed by.
    name: &'static str,
    /// A path whose recursive read reports this parameter, or `None` for a name that
    /// sits in no hierarchy (`GetParametersByPath` reads a hierarchy, so a plain name
    /// has no path read to lose it from).
    path: Option<&'static str>,
    /// The parameter's `ParameterType`.
    parameter_type: &'static str,
    /// The values written, in order: more than one means that many retained versions
    /// when the delete runs.
    values: &'static [&'static str],
    /// The tags the create carried, which the delete must take with the parameter.
    tags: &'static [(&'static str, &'static str)],
}

/// A tag the rows share with the bystander below, so a tag-filtered read after a delete
/// proves the delete dropped THAT parameter's tag rather than the whole tag index.
const SHARED_TAG: (&str, &str) = ("Owner", "platform");

/// Written once and never deleted: every assertion that a report lost a deleted name is
/// paired with one that this parameter is still reported.
const BYSTANDER: &str = "/visibility/kept/untouched";

/// The path whose recursive read reports [`BYSTANDER`].
const BYSTANDER_PATH: &str = "/visibility/kept";

const ROWS: &[Row] = &[
    Row {
        case: "a plain name, one version, untagged",
        name: "plain-visibility-name",
        path: None,
        parameter_type: "String",
        values: &["only"],
        tags: &[],
    },
    Row {
        case: "a two-level hierarchy name, three versions, one tag",
        name: "/visibility/config",
        path: Some("/visibility"),
        parameter_type: "String",
        values: &["one", "two", "three"],
        tags: &[SHARED_TAG],
    },
    Row {
        case: "a four-level SecureString, two versions, two tags",
        name: "/visibility/secrets/db/password",
        path: Some("/visibility/secrets"),
        parameter_type: "SecureString",
        values: &["s3cr3t", "r0tated"],
        tags: &[SHARED_TAG, ("Stage", "test")],
    },
    Row {
        case: "a StringList, one version, one tag of its own",
        name: "/visibility/lists/subnets",
        path: Some("/visibility/lists"),
        parameter_type: "StringList",
        values: &["subnet-a,subnet-b"],
        tags: &[("Stage", "test")],
    },
];

/// Every reader of the slice stops reporting a deleted parameter, and keeps reporting
/// the one the delete did not address.
#[tokio::test]
async fn a_delete_takes_the_parameter_out_of_every_report() {
    let router = router();
    seed_bystander(&router).await;

    for row in ROWS {
        let case = row.case;
        write_row(&router, row).await;

        // Before the delete, every reader holds it — so what the asserts below observe
        // is the delete and not a parameter that was never there.
        let found = describe_names(&router, case, &by_name(row.name)).await;
        assert!(
            found.iter().any(|name| name == row.name),
            "case {case:?}: expected DescribeParameters to report {:?} before the delete, got \
             {found:?}",
            row.name
        );
        let (status, history) = call(
            &router,
            "GetParameterHistory",
            &json!({ "Name": row.name }),
        )
        .await;
        assert_eq!(
            (status, reported_names(&history).len()),
            (200, row.values.len()),
            "case {case:?}: expected {} retained version(s) before the delete, got {status} \
             {history}",
            row.values.len()
        );
        for (key, value) in row.tags {
            let tagged = describe_names(&router, case, &by_tag(key, value)).await;
            assert!(
                tagged.iter().any(|name| name == row.name),
                "case {case:?}: expected the tag read {key}={value} to report {:?} before the \
                 delete, got {tagged:?}",
                row.name
            );
        }
        if let Some(path) = row.path {
            let under = path_names(&router, case, path).await;
            assert!(
                under.iter().any(|name| name == row.name),
                "case {case:?}: expected the path read of {path:?} to report {:?} before the \
                 delete, got {under:?}",
                row.name
            );
        }

        let (status, deleted) = call(&router, "DeleteParameter", &json!({ "Name": row.name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteParameter, got {status} {deleted}"
        );

        // The history of every retained version is the modelled refusal. (The value read
        // and the repeated delete are pinned by `all/parameter_deletion.rs`.)
        let body = json!({ "Name": row.name });
        let (status, refused) = call(&router, "GetParameterHistory", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ParameterNotFound"),
            "case {case:?}: expected GetParameterHistory {body} to answer 400 ParameterNotFound \
             after the delete, got {status} {refused}"
        );

        // The metadata reads report it no more — neither by its name nor by its tags —
        // while the bystander that shares a tag with it is still reported.
        let gone = describe_names(&router, case, &by_name(row.name)).await;
        assert!(
            gone.is_empty(),
            "case {case:?}: expected DescribeParameters to report nothing for {:?} after the \
             delete, got {gone:?}",
            row.name
        );
        for (key, value) in row.tags {
            let tagged = describe_names(&router, case, &by_tag(key, value)).await;
            assert!(
                !tagged.iter().any(|name| name == row.name),
                "case {case:?}: expected the tag read {key}={value} to have lost {:?}, got \
                 {tagged:?}",
                row.name
            );
        }
        if let Some(path) = row.path {
            let under = path_names(&router, case, path).await;
            assert!(
                !under.iter().any(|name| name == row.name),
                "case {case:?}: expected the path read of {path:?} to have lost {:?}, got \
                 {under:?}",
                row.name
            );
        }

        assert_bystander_survives(&router, case).await;
    }
}

/// A name written again after its parameter was deleted is a NEW parameter: the write
/// needs no `Overwrite` (nothing holds the name), it starts at version 1, its history
/// holds that one version, and it carries none of the deleted parameter's tags.
#[tokio::test]
async fn a_name_written_again_after_a_delete_is_a_fresh_parameter() {
    let router = router();
    seed_bystander(&router).await;

    for row in ROWS {
        let case = row.case;
        write_row(&router, row).await;

        // While the name is held, a write without `Overwrite` is refused — which is what
        // makes the same write succeeding after the delete evidence of the removal.
        let (status, refused) = call(
            &router,
            "PutParameter",
            &json!({ "Name": row.name, "Type": row.parameter_type, "Value": "rewritten" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ParameterAlreadyExists"),
            "case {case:?}: expected a no-Overwrite write of a held name to be refused, got \
             {status} {refused}"
        );

        let (status, deleted) = call(&router, "DeleteParameter", &json!({ "Name": row.name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteParameter, got {status} {deleted}"
        );

        let (status, written) = call(
            &router,
            "PutParameter",
            &json!({ "Name": row.name, "Type": row.parameter_type, "Value": "rewritten" }),
        )
        .await;
        assert_eq!(
            (status, written.get("Version").and_then(Value::as_i64)),
            (200, Some(1)),
            "case {case:?}: expected the write after the delete to mint version 1, got {status} \
             {written}"
        );

        // A `SecureString` is stored as the ciphertext its put sealed, so the read that
        // compares the value with what the write supplied asks for it decrypted; the flag
        // "is ignored for String and StringList parameter types".
        let (status, read) = call(
            &router,
            "GetParameter",
            &json!({ "Name": row.name, "WithDecryption": true }),
        )
        .await;
        let parameter = &read["Parameter"];
        assert_eq!(
            (
                status,
                parameter.get("Version").and_then(Value::as_i64),
                parameter.get("Value").and_then(Value::as_str),
            ),
            (200, Some(1), Some("rewritten")),
            "case {case:?}: expected the fresh parameter to read back at version 1, got {status} \
             {read}"
        );

        let (status, history) = call(
            &router,
            "GetParameterHistory",
            &json!({ "Name": row.name }),
        )
        .await;
        assert_eq!(
            (status, reported_names(&history).len()),
            (200, 1),
            "case {case:?}: expected the fresh parameter to hold exactly one version, got \
             {status} {history}"
        );

        // The deleted parameter's tags went with it, so the fresh one is not reported
        // under them — the bystander still is.
        for (key, value) in row.tags {
            let tagged = describe_names(&router, case, &by_tag(key, value)).await;
            assert!(
                !tagged.iter().any(|name| name == row.name),
                "case {case:?}: expected the re-written {:?} to carry none of the deleted \
                 parameter's tags, got {key}={value} reporting {tagged:?}",
                row.name
            );
        }

        assert_bystander_survives(&router, case).await;
    }
}

/// Write [`BYSTANDER`], tagged with [`SHARED_TAG`].
async fn seed_bystander(router: &MultiServiceRouter) {
    let (key, value) = SHARED_TAG;
    let (status, response) = call(
        router,
        "PutParameter",
        &json!({
            "Name": BYSTANDER,
            "Type": "String",
            "Value": "kept",
            "Tags": [{ "Key": key, "Value": value }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the bystander\": expected PutParameter to succeed, got {status} {response}"
    );
}

/// Write every version of `row`, the first carrying its tags and the rest overwriting.
async fn write_row(router: &MultiServiceRouter, row: &Row) {
    for (index, value) in row.values.iter().enumerate() {
        let mut request = json!({
            "Name": row.name,
            "Type": row.parameter_type,
            "Value": value,
        });
        if index == 0 {
            if !row.tags.is_empty() {
                request["Tags"] = Value::Array(
                    row.tags
                        .iter()
                        .map(|(key, value)| json!({ "Key": key, "Value": value }))
                        .collect(),
                );
            }
        } else {
            request["Overwrite"] = Value::Bool(true);
        }
        let (status, response) = call(router, "PutParameter", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutParameter {request} to succeed, got {status} {response}",
            row.case
        );
    }
}

/// The bystander still reads back, and both the tag read and the path read that reported
/// it before any delete still do.
async fn assert_bystander_survives(router: &MultiServiceRouter, case: &str) {
    let (status, kept) = call(router, "GetParameter", &json!({ "Name": BYSTANDER })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the bystander to survive the delete, got {status} {kept}"
    );

    let (key, value) = SHARED_TAG;
    let tagged = describe_names(router, case, &by_tag(key, value)).await;
    assert!(
        tagged.iter().any(|name| name == BYSTANDER),
        "case {case:?}: expected the tag read {key}={value} to still report the bystander, got \
         {tagged:?}"
    );

    let under = path_names(router, case, BYSTANDER_PATH).await;
    assert!(
        under.iter().any(|name| name == BYSTANDER),
        "case {case:?}: expected the path read of {BYSTANDER_PATH:?} to still report the \
         bystander, got {under:?}"
    );
}

/// A `DescribeParameters` body narrowing the report to one name.
fn by_name(name: &str) -> Value {
    json!({ "ParameterFilters": [{ "Key": "Name", "Values": [name] }] })
}

/// A `DescribeParameters` body narrowing the report to the parameters carrying one tag.
fn by_tag(key: &str, value: &str) -> Value {
    json!({ "ParameterFilters": [{ "Key": format!("tag:{key}"), "Values": [value] }] })
}

/// The names `DescribeParameters` reports for `body`, asserting the read succeeded.
async fn describe_names(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<String> {
    let (status, response) = call(router, "DescribeParameters", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeParameters {body} to succeed, got {status} {response}"
    );
    reported_names(&response)
}

/// The names a recursive `GetParametersByPath` of `path` reports, asserting it succeeded.
async fn path_names(router: &MultiServiceRouter, case: &str, path: &str) -> Vec<String> {
    let body = json!({ "Path": path, "Recursive": true });
    let (status, response) = call(router, "GetParametersByPath", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetParametersByPath {body} to succeed, got {status} {response}"
    );
    reported_names(&response)
}

/// The `Name` of every parameter a response's `Parameters` list reports, in order.
fn reported_names(response: &Value) -> Vec<String> {
    response["Parameters"]
        .as_array()
        .map(|parameters| {
            parameters
                .iter()
                .filter_map(|parameter| parameter["Name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
