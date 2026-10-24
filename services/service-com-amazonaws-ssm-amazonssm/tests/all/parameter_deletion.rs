// SPDX-License-Identifier: CC0-1.0

//! `DeleteParameter` removes one parameter from Parameter Store, end to end over the
//! front door.
//!
//! The rule under test: **a delete removes exactly the parameter its `Name` resolves to —
//! every retained version of it — so a later read of that name is the modelled
//! `ParameterNotFound` while every other parameter is untouched; a name the store does not
//! hold is that same refusal rather than a success.**
//!
//! The table's rows differ along what the rule generalises over: the name's form (a plain
//! name, a hierarchy path, a path written with and without its leading `/`), how many
//! versions the parameter carries, its type, and the Region it was written in. Each row
//! writes, reads, deletes and reads again, so nothing passes on a response that is not
//! computed from the store.
//!
//! The front-door harness is this crate's one harness (`all/parameter_store.rs`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, REGION};

/// One row: the name a parameter is written under, how it is addressed, and what it is.
struct Row {
    case: &'static str,
    /// The name `PutParameter` writes the parameter under.
    written: &'static str,
    /// The name `DeleteParameter` and `GetParameter` address it by — the same parameter,
    /// since "the leading slash is not part of the name" for a hierarchy path.
    addressed: &'static str,
    /// The parameter's type.
    parameter_type: &'static str,
    /// The values written, in order: more than one means the parameter carries that many
    /// retained versions when the delete runs.
    values: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a plain name, one version",
        written: "plain-name",
        addressed: "plain-name",
        parameter_type: "String",
        values: &["only"],
    },
    Row {
        case: "a hierarchy path, three versions",
        written: "/team/service/config",
        addressed: "/team/service/config",
        parameter_type: "String",
        values: &["one", "two", "three"],
    },
    Row {
        case: "a hierarchy path addressed without its leading slash",
        written: "/cdk-bootstrap/hnb659fds/version",
        addressed: "cdk-bootstrap/hnb659fds/version",
        parameter_type: "String",
        values: &["28"],
    },
    Row {
        case: "a StringList parameter, two versions",
        written: "/lists/subnets",
        addressed: "/lists/subnets",
        parameter_type: "StringList",
        values: &["a,b", "a,b,c"],
    },
];

/// A parameter a name resolves to is deleted whole, and reading it afterwards is
/// `ParameterNotFound` — while the parameters the row did not delete still read back.
#[tokio::test]
async fn a_delete_removes_the_parameter_its_name_resolves_to() {
    let router = router();

    // Written once and never deleted: the delete must leave it alone.
    let bystander = "/kept/untouched";
    put_ok(
        &router,
        "the bystander",
        &json!({ "Name": bystander, "Type": "String", "Value": "kept" }),
    )
    .await;

    for row in ROWS {
        let case = row.case;
        for (index, value) in row.values.iter().enumerate() {
            let mut request = json!({
                "Name": row.written,
                "Type": row.parameter_type,
                "Value": value,
            });
            if index > 0 {
                request["Overwrite"] = Value::Bool(true);
            }
            put_ok(&router, case, &request).await;
        }

        let (status, read) = call(&router, "GetParameter", &json!({ "Name": row.addressed })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the written parameter to read back, got {status} {read}"
        );

        let (status, deleted) =
            call(&router, "DeleteParameter", &json!({ "Name": row.addressed })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteParameter, got {status} {deleted}"
        );

        // Every version is gone with the parameter: the latest, and the first by selector.
        for addressed in [
            row.addressed.to_string(),
            format!("{}:1", row.addressed),
            row.written.to_string(),
        ] {
            let (status, gone) =
                call(&router, "GetParameter", &json!({ "Name": addressed })).await;
            assert_eq!(
                (status, error_code(&gone).as_str()),
                (400, "ParameterNotFound"),
                "case {case:?}: expected 400 ParameterNotFound for {addressed:?}, got {status} \
                 {gone}"
            );
        }

        // A second delete of the same name is the same refusal: "for the DeleteParameter
        // and GetParameter actions, if the specified parameter doesn't exist, the
        // ParameterNotFound exception" is what is answered.
        let (status, again) =
            call(&router, "DeleteParameter", &json!({ "Name": row.addressed })).await;
        assert_eq!(
            (status, error_code(&again).as_str()),
            (400, "ParameterNotFound"),
            "case {case:?}: expected a second delete to answer 400 ParameterNotFound, got \
             {status} {again}"
        );

        let (status, kept) = call(&router, "GetParameter", &json!({ "Name": bystander })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the bystander parameter to survive the delete, got \
             {status} {kept}"
        );
    }
}

/// A parameter lives in the Region its endpoint names, so a delete signed for another
/// Region does not reach it: that Region holds no such parameter, and the one that does
/// still reads back.
#[tokio::test]
async fn a_delete_reaches_only_the_region_it_is_signed_for() {
    let router = router();
    let case = "one name written in two Regions";
    let name = "/per-region/value";
    let other = "eu-central-1";

    for (region, value) in [(REGION, "home"), (other, "away")] {
        let (status, written) = call_in_region(
            &router,
            region,
            "PutParameter",
            &json!({ "Name": name, "Type": "String", "Value": value }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutParameter in {region} to succeed, got {status} {written}"
        );
    }

    let (status, deleted) =
        call_in_region(&router, other, "DeleteParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete in {other} to succeed, got {status} {deleted}"
    );

    let (status, gone) =
        call_in_region(&router, other, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        (status, error_code(&gone).as_str()),
        (400, "ParameterNotFound"),
        "case {case:?}: expected the deleted Region's read to answer 400 ParameterNotFound, \
         got {status} {gone}"
    );

    let (status, kept) = call(&router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other Region's parameter to survive, got {status} {kept}"
    );
    assert_eq!(
        kept.get("Parameter")
            .and_then(|parameter| parameter.get("Value"))
            .and_then(Value::as_str),
        Some("home"),
        "case {case:?}: expected the Region's own value, got {kept}"
    );
}

/// A delete that addresses a VERSION rather than the parameter names nothing this
/// operation can remove: `DeleteParameter` takes "the name of the parameter to delete",
/// not a version selector, so the parameter is left as it is.
#[tokio::test]
async fn a_delete_addressing_a_version_removes_nothing() {
    let router = router();
    let name = "/selector/parameter";
    put_ok(
        &router,
        "the parameter under test",
        &json!({ "Name": name, "Type": "String", "Value": "one" }),
    )
    .await;

    for addressed in [
        format!("{name}:1"),
        format!("{name}:label"),
        format!("arn:aws:ssm:{REGION}:{}:parameter{name}", engine::EMULATOR_ACCOUNT_ID),
    ] {
        let (status, refused) =
            call(&router, "DeleteParameter", &json!({ "Name": addressed })).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ParameterNotFound"),
            "case {addressed:?}: expected 400 ParameterNotFound, got {status} {refused}"
        );
    }

    let (status, kept) = call(&router, "GetParameter", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case \"the parameter after every refused delete\": expected it to still read back, \
         got {status} {kept}"
    );
}

/// `PutParameter`, asserting it succeeded.
async fn put_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutParameter to succeed, got {status} {response}"
    );
}
