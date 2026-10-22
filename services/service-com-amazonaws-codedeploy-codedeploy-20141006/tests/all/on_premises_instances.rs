// SPDX-License-Identifier: CC0-1.0

//! On-premises instance registration, driven over the wire.
//!
//! RULE under test: `RegisterOnPremisesInstance` binds EXACTLY ONE IAM ARN (an
//! IAM user ARN or an IAM session ARN, never both and never neither) to an
//! on-premises instance name and persists that registration, so a later
//! `GetOnPremisesInstance` answers the stored record; a request that breaks one
//! of the operation's preconditions is refused with HTTP 400 and the modelled
//! error for that precondition, and persists nothing.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// An IAM user ARN: `arn:aws:iam::<account>:user/<name>`.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", account())
}

/// An IAM session ARN: `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
fn session_arn(role: &str, session: &str) -> String {
    format!("arn:aws:sts::{}:assumed-role/{role}/{session}", account())
}

/// The `instanceArn` the emulator reports for `name`:
/// `arn:aws:codedeploy:<region>:<account>:instance/<name>`. The harness signs
/// nothing, so the region is the emulator's default.
fn expected_instance_arn(name: &str) -> String {
    format!("arn:aws:codedeploy:us-east-1:{}:instance/{name}", account())
}

/// Read `name` back, requiring the read to succeed.
async fn get_instance(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": name}))
        .await["instanceInfo"]
        .clone()
}

/// RULE: a registration is persisted and read back from the stored record —
/// the name, the ONE IAM ARN it was made with (and only that one), the
/// instance's ARN and a registration time — whatever the accepted name and
/// whichever IAM ARN type the request used.
#[tokio::test]
async fn a_registration_reads_back_as_it_was_made() {
    let emulator = Emulator::new();
    // Rows differ along the IAM ARN TYPE and the FORM of an accepted name:
    // plain, every punctuation character CodeDeploy's resource-name pattern
    // allows (`smithy_query(query="shape",shape="ApplicationName")`:
    // `^[A-Za-z0-9+=,.@_-]*$`), its published maximum length (100), and the
    // shortest name there is.
    let max_length_name = "n".repeat(100);
    let cases: &[(&str, bool)] = &[
        ("onprem-one", true),
        ("onprem-session", false),
        ("a+b=c,d.e@f_g-h", true),
        (max_length_name.as_str(), false),
        ("i", true),
    ];

    for (name, by_user_arn) in cases {
        let (member, arn) = if *by_user_arn {
            ("iamUserArn", user_arn(name))
        } else {
            ("iamSessionArn", session_arn("OnPremRole", name))
        };
        let mut request = json!({"instanceName": name});
        request[member] = json!(arn);
        emulator.ok("RegisterOnPremisesInstance", request).await;

        let info = get_instance(&emulator, name).await;
        assert_eq!(
            info["instanceName"].as_str(),
            Some(*name),
            "case {name:?}: expected the stored instanceName, got {info:?}"
        );
        assert_eq!(
            info[member].as_str(),
            Some(arn.as_str()),
            "case {name:?}: expected {member} {arn}, got {info:?}"
        );
        // Exactly one IAM ARN: the member the request did not send is absent.
        let other = if *by_user_arn {
            "iamSessionArn"
        } else {
            "iamUserArn"
        };
        assert!(
            info[other].is_null(),
            "case {name:?}: expected no {other}, got {info:?}"
        );
        assert_eq!(
            info["instanceArn"].as_str(),
            Some(expected_instance_arn(name).as_str()),
            "case {name:?}: expected the instance ARN, got {info:?}"
        );
        assert!(
            info["registerTime"].as_f64().unwrap_or_default() > 0.0,
            "case {name:?}: expected a registerTime, got {info:?}"
        );
        assert!(
            info["deregisterTime"].is_null(),
            "case {name:?}: expected no deregisterTime on a registered instance, got {info:?}"
        );

        // A second read answers the same record: reporting changes nothing.
        let again = get_instance(&emulator, name).await;
        assert_eq!(
            again, info,
            "case {name:?}: expected a read to be repeatable, got {again:?}"
        );
    }
}

/// RULE: a request that breaks one of the operation's preconditions is refused
/// with HTTP 400 and the modelled error for THAT precondition, and persists
/// nothing — so the name it named is still not registered afterwards.
#[tokio::test]
async fn a_refused_registration_names_its_precondition_and_stores_nothing() {
    let emulator = Emulator::new();
    // Rows differ along WHICH precondition the request breaks: the IAM-ARN
    // cardinality ("Only one IAM ARN (an IAM session ARN or IAM user ARN) is
    // supported in the request. You cannot use both"), the form of each ARN
    // type, and the form of the instance name — including one character past
    // CodeDeploy's published 100-character name bound.
    let too_long_name = "n".repeat(101);
    let cases: &[(&str, Value, &str)] = &[
        (
            "no IAM ARN at all",
            json!({"instanceName": "refused-none"}),
            "IamArnRequiredException",
        ),
        (
            "an empty IAM user ARN",
            json!({"instanceName": "refused-empty", "iamUserArn": ""}),
            "IamArnRequiredException",
        ),
        (
            "both IAM ARN types",
            json!({
                "instanceName": "refused-both",
                "iamUserArn": user_arn("both"),
                "iamSessionArn": session_arn("OnPremRole", "both"),
            }),
            "MultipleIamArnsProvidedException",
        ),
        (
            "a user ARN that is not an ARN",
            json!({"instanceName": "refused-user-1", "iamUserArn": "not-an-arn"}),
            "InvalidIamUserArnException",
        ),
        (
            "a user ARN of another service",
            json!({
                "instanceName": "refused-user-2",
                "iamUserArn": format!("arn:aws:sts::{}:user/u", account()),
            }),
            "InvalidIamUserArnException",
        ),
        (
            "an IAM ARN that is not a user",
            json!({
                "instanceName": "refused-user-3",
                "iamUserArn": format!("arn:aws:iam::{}:role/r", account()),
            }),
            "InvalidIamUserArnException",
        ),
        (
            "a user ARN with no user name",
            json!({
                "instanceName": "refused-user-4",
                "iamUserArn": format!("arn:aws:iam::{}:user/", account()),
            }),
            "InvalidIamUserArnException",
        ),
        (
            "a user ARN with no account",
            json!({"instanceName": "refused-user-5", "iamUserArn": "arn:aws:iam:::user/u"}),
            "InvalidIamUserArnException",
        ),
        (
            "a session ARN that is not an ARN",
            json!({"instanceName": "refused-session-1", "iamSessionArn": "session"}),
            "InvalidIamSessionArnException",
        ),
        (
            "a session ARN that is an IAM user ARN",
            json!({
                "instanceName": "refused-session-2",
                "iamSessionArn": user_arn("not-a-session"),
            }),
            "InvalidIamSessionArnException",
        ),
        (
            "a session ARN with no session",
            json!({
                "instanceName": "refused-session-3",
                "iamSessionArn": format!("arn:aws:sts::{}:assumed-role/", account()),
            }),
            "InvalidIamSessionArnException",
        ),
        (
            "an empty instance name",
            json!({"instanceName": "", "iamUserArn": user_arn("empty-name")}),
            "InstanceNameRequiredException",
        ),
        (
            "a blank instance name",
            json!({"instanceName": "   ", "iamUserArn": user_arn("blank-name")}),
            "InstanceNameRequiredException",
        ),
        (
            "an instance name one character past the 100-character bound",
            json!({"instanceName": too_long_name, "iamUserArn": user_arn("long-name")}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance name carrying a character outside the name charset",
            json!({"instanceName": "onprem/slash", "iamUserArn": user_arn("slash-name")}),
            "InvalidInstanceNameException",
        ),
        (
            "an instance name carrying a space",
            json!({"instanceName": "onprem name", "iamUserArn": user_arn("space-name")}),
            "InvalidInstanceNameException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("RegisterOnPremisesInstance", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *expected,
            "case {case:?}: expected {expected}, got {body:?}"
        );

        // Nothing was persisted: the name the refused request named is still
        // unregistered. A name the service will not even accept is skipped —
        // reading it back is refused for the same reason.
        let name = request["instanceName"].as_str().unwrap_or_default();
        if error_code(&body).ends_with("InstanceNameRequiredException")
            || error_code(&body).ends_with("InvalidInstanceNameException")
        {
            continue;
        }
        let (read_status, read_body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: expected {name} to be unregistered, got {read_status} {read_body:?}"
        );
        assert_eq!(
            error_code(&read_body),
            "InstanceNotRegisteredException",
            "case {case:?}: expected InstanceNotRegisteredException for {name}, got {read_body:?}"
        );
    }
}

/// RULE: a name and an IAM ARN each back at most ONE registered instance, so a
/// second registration of either is refused with the modelled error for it and
/// leaves the first registration's record untouched.
#[tokio::test]
async fn a_name_or_an_iam_arn_backs_one_registered_instance() {
    let emulator = Emulator::new();
    // Rows differ along the IAM ARN type, and along the conflict: the same name
    // again, or the same IAM ARN under a different name.
    let cases: &[(&str, &str, bool, &str)] = &[
        ("by user ARN", "held-by-user", true, "IamUserArnAlreadyRegisteredException"),
        (
            "by session ARN",
            "held-by-session",
            false,
            "IamSessionArnAlreadyRegisteredException",
        ),
    ];

    for (case, name, by_user_arn, arn_conflict_code) in cases {
        let (member, arn) = if *by_user_arn {
            ("iamUserArn", user_arn(name))
        } else {
            ("iamSessionArn", session_arn("OnPremRole", name))
        };
        let mut request = json!({"instanceName": name});
        request[member] = json!(arn);
        emulator
            .ok("RegisterOnPremisesInstance", request.clone())
            .await;
        let registered = get_instance(&emulator, name).await;

        // The same name again, with a DIFFERENT IAM ARN of the same type.
        let mut same_name = json!({"instanceName": name});
        same_name[member] = json!(if *by_user_arn {
            user_arn("other-identity")
        } else {
            session_arn("OtherRole", "other-identity")
        });
        let (status, body) = emulator
            .call("RegisterOnPremisesInstance", same_name)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 re-registering {name}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InstanceNameAlreadyRegisteredException",
            "case {case:?}: expected InstanceNameAlreadyRegisteredException, got {body:?}"
        );

        // The same IAM ARN under a different name.
        let other_name = format!("{name}-second");
        let mut same_arn = json!({"instanceName": other_name});
        same_arn[member] = json!(arn);
        let (status, body) = emulator.call("RegisterOnPremisesInstance", same_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 reusing {arn}, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            *arn_conflict_code,
            "case {case:?}: expected {arn_conflict_code}, got {body:?}"
        );

        // Neither refusal touched the stored registration, and the name the
        // second request named was never registered.
        let after = get_instance(&emulator, name).await;
        assert_eq!(
            after, registered,
            "case {case:?}: expected the first registration to survive, got {after:?}"
        );
        let (read_status, read_body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": other_name}))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case:?}: expected {other_name} to be unregistered, got {read_body:?}"
        );
        assert_eq!(
            error_code(&read_body),
            "InstanceNotRegisteredException",
            "case {case:?}: expected InstanceNotRegisteredException, got {read_body:?}"
        );
    }
}

/// RULE: the registry is BOUNDED — a loop of registrations cannot grow the
/// store without limit, so once it holds as many registered instances as it
/// keeps, a further registration is refused with HTTP 400 and the ones already
/// recorded are still readable.
#[tokio::test]
async fn the_registry_is_bounded() {
    let emulator = Emulator::new();
    let maximum = service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_ON_PREMISES_INSTANCES;
    for index in 0..maximum {
        let name = format!("bounded-{index}");
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({"instanceName": name, "iamUserArn": user_arn(&name)}),
            )
            .await;
    }

    // One past the bound: refused, and refused as a client error rather than
    // accepted into a store that would keep growing.
    let name = format!("bounded-{maximum}");
    let (status, body) = emulator
        .call(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(&name)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {name:?}: expected HTTP 400 past the {maximum}-instance bound, got {status} {body:?}"
    );
    let (read_status, read_body) = emulator
        .call("GetOnPremisesInstance", json!({"instanceName": name}))
        .await;
    assert_eq!(
        read_status, 400,
        "case {name:?}: expected it never to be registered, got {read_status} {read_body:?}"
    );
    assert_eq!(
        error_code(&read_body),
        "InstanceNotRegisteredException",
        "case {name:?}: expected InstanceNotRegisteredException, got {read_body:?}"
    );

    // The bound dropped nothing that is still registered: the oldest and the
    // newest accepted registrations both read back.
    for name in ["bounded-0", &format!("bounded-{}", maximum - 1)] {
        let info = get_instance(&emulator, name).await;
        assert_eq!(
            info["instanceName"].as_str(),
            Some(name),
            "case {name:?}: expected it to survive the bound, got {info:?}"
        );
    }
}

/// RULE: a registration is recorded per name, so instances registered in one
/// store do not borrow each other's identity — each reads back its own IAM ARN
/// and its own ARN.
#[tokio::test]
async fn registrations_are_independent_records() {
    let emulator = Emulator::new();
    // Rows differ along count: several registrations live in the one store.
    let names: &[&str] = &["fleet-a", "fleet-b", "fleet-c"];
    for name in names {
        emulator
            .ok(
                "RegisterOnPremisesInstance",
                json!({"instanceName": name, "iamUserArn": user_arn(name)}),
            )
            .await;
    }

    for name in names {
        let info = get_instance(&emulator, name).await;
        assert_eq!(
            info["iamUserArn"].as_str(),
            Some(user_arn(name).as_str()),
            "case {name:?}: expected its own iamUserArn, got {info:?}"
        );
        assert_eq!(
            info["instanceArn"].as_str(),
            Some(expected_instance_arn(name).as_str()),
            "case {name:?}: expected its own instanceArn, got {info:?}"
        );
    }

    // A name nobody registered is not registered, however many are.
    let (status, body) = emulator
        .call("GetOnPremisesInstance", json!({"instanceName": "fleet-ghost"}))
        .await;
    assert_eq!(status, 400, "expected HTTP 400 for a ghost, got {body:?}");
    assert_eq!(
        error_code(&body),
        "InstanceNotRegisteredException",
        "expected InstanceNotRegisteredException, got {body:?}"
    );
}
