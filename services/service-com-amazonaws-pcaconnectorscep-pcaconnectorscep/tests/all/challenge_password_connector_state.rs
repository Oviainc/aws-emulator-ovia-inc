// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for reading a challenge password while the owning connector is not serving.
//!
//! The rule under test: the challenge password is the secret the owning connector
//! authenticates a certificate request with, so it is handed out only while that connector is
//! in the one status that lets it serve one (`ACTIVE`). A challenge whose connector is in any
//! other status — a `DeleteConnector` already accepted and the connector reported `DELETING`,
//! or the record gone once that teardown came due — is not a resource `GetChallengePassword`
//! can retrieve: it answers HTTP 404 `ResourceNotFoundException`, the error whose own doc
//! covers a resource that "might have a status other than `ACTIVE`" (smithy_query: doc
//! ResourceNotFoundException), naming the requested ARN and the `Challenge` resource kind, and
//! the response carries no password. That is the SAME precondition the sibling that MINTS a
//! password holds a caller to, so the password cannot be reached through the reader after the
//! create has begun refusing it.
//!
//! What does NOT change: the connector itself and the challenge's METADATA keep being reported
//! for the whole teardown window — only the password is withheld — and a read aimed at a
//! challenge of any other connector is untouched by one connector's teardown.
//!
//! The rows differ along what the rule generalises over: the status the owning connector is in
//! when the password is read, how many challenges that connector owns and which of them the
//! row reads, whether the challenge carries tags, and which Region the whole case runs in.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// The service's own status and resource-kind vocabulary, named from its constants so a case
/// cannot drift off them.
const STATUS_ACTIVE: &str = scep::state::STATUS_ACTIVE;
const STATUS_DELETING: &str = scep::state::STATUS_DELETING;
const RESOURCE_TYPE_CHALLENGE: &str = scep::state::RESOURCE_TYPE_CHALLENGE;

/// Two Regions, neither the emulator default, so every ARN comes from the request's own SigV4
/// credential scope and no row can pass by a Region baked into the service.
const REGIONS: [&str; 2] = ["eu-west-1", "ap-southeast-2"];

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern, in `region`.
fn ca_arn(region: &str) -> String {
    format!(
        "arn:aws:acm-pca:{region}:000000000000:certificate-authority/\
         11111111-2222-3333-4444-555555555555"
    )
}

/// The status the owning connector is in when the row reads the password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Settled and serving.
    Active,
    /// `DeleteConnector` accepted, teardown still running: the record is reported `DELETING`.
    Deleting,
    /// The teardown has come due, so the connector and its challenges are gone.
    ToreDown,
}

#[derive(Debug)]
struct Case {
    name: &'static str,
    region: &'static str,
    owner: Owner,
    /// How many challenges the owning connector is given.
    challenges: usize,
    /// Which of them the row reads the password of.
    read: usize,
    /// The tags that challenge was created with.
    tags: &'static [(&'static str, &'static str)],
}

/// The rows: the serving status and both non-serving ones, connectors owning one challenge and
/// several, the first and the last of those read, tagged and untagged challenges, and both
/// Regions.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "serving connector, its only challenge",
            region: REGIONS[0],
            owner: Owner::Active,
            challenges: 1,
            read: 0,
            tags: &[],
        },
        Case {
            name: "serving connector, the last of three challenges, tagged",
            region: REGIONS[1],
            owner: Owner::Active,
            challenges: 3,
            read: 2,
            tags: &[("owner", "platform"), ("stage", "one")],
        },
        Case {
            name: "teardown accepted, its only challenge",
            region: REGIONS[0],
            owner: Owner::Deleting,
            challenges: 1,
            read: 0,
            tags: &[],
        },
        Case {
            name: "teardown accepted, the first of three challenges",
            region: REGIONS[1],
            owner: Owner::Deleting,
            challenges: 3,
            read: 0,
            tags: &[],
        },
        Case {
            name: "teardown accepted, the last of three challenges, tagged",
            region: REGIONS[0],
            owner: Owner::Deleting,
            challenges: 3,
            read: 2,
            tags: &[("zone", "b")],
        },
        Case {
            name: "teardown come due, the record is gone",
            region: REGIONS[1],
            owner: Owner::ToreDown,
            challenges: 2,
            read: 1,
            tags: &[],
        },
    ]
}

async fn create_connector(router: &MultiServiceRouter, region: &str) -> Answer {
    harness::call(
        router,
        region,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca_arn(region) })),
    )
    .await
}

async fn create_challenge(
    router: &MultiServiceRouter,
    region: &str,
    connector: &str,
    tags: &[(&str, &str)],
) -> Answer {
    let mut request = json!({ "ConnectorArn": connector });
    if !tags.is_empty() {
        request["Tags"] = Value::Object(
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
                .collect(),
        );
    }
    harness::call(router, region, "POST", "/challenges", Some(request)).await
}

async fn delete_connector(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn get_connector(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn get_challenge_metadata(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/challengeMetadata/{}", label(arn)),
        None,
    )
    .await
}

async fn get_challenge_password(router: &MultiServiceRouter, region: &str, arn: &str) -> Answer {
    harness::call(
        router,
        region,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

/// Stand a connector up in `region`, settled and serving, and answer its ARN.
async fn serving_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    name: &str,
) -> String {
    let created = create_connector(router, region).await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let connector = created.text("ConnectorArn");
    harness::release_pending(engine, &connector);
    let read = get_connector(router, region, &connector).await;
    assert_eq!(
        read.connector("Status"),
        &json!(STATUS_ACTIVE),
        "case {name:?}: expected the connector to be {STATUS_ACTIVE} before the case, got {:?}",
        read.body
    );
    connector
}

/// Mint `count` challenges on `connector`, giving the one at `tagged` the row's tags, and
/// answer each challenge's `(ARN, password)` as the create reported it.
async fn mint_challenges(
    router: &MultiServiceRouter,
    region: &str,
    connector: &str,
    count: usize,
    tagged: usize,
    tags: &[(&str, &str)],
    name: &str,
) -> Vec<(String, String)> {
    let mut minted = Vec::new();
    for index in 0..count {
        let created = create_challenge(
            router,
            region,
            connector,
            if index == tagged { tags } else { &[] },
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: expected 202 from CreateChallenge #{index}, got {} / {:?}",
            created.status, created.body
        );
        let arn = created
            .challenge("Arn")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected a challenge Arn in {:?}", created.body)
            })
            .to_string();
        let password = created
            .challenge("Password")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected a challenge Password in {:?}", created.body)
            })
            .to_string();
        minted.push((arn, password));
    }
    minted
}

/// Assert `answer` is the modelled 404 for `challenge_arn` and hands no password out.
fn assert_withheld(answer: &Answer, name: &str, challenge_arn: &str, password: &str, what: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "ResourceNotFoundException"),
        "case {name:?}: {what} must answer 404 ResourceNotFoundException, got {} / {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        (&answer.body["ResourceId"], &answer.body["ResourceType"]),
        (&json!(challenge_arn), &json!(RESOURCE_TYPE_CHALLENGE)),
        "case {name:?}: {what} must name the requested ARN and the {RESOURCE_TYPE_CHALLENGE} \
         kind, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["Password"],
        Value::Null,
        "case {name:?}: {what} must carry no Password, got {:?}",
        answer.body
    );
    let raw = String::from_utf8_lossy(&answer.raw).to_string();
    assert!(
        !raw.contains(password),
        "case {name:?}: {what} must not leak the challenge password, got {raw:?}"
    );
}

#[tokio::test]
async fn a_challenge_password_is_served_only_while_its_connector_is_serving() {
    for case in cases() {
        let name = case.name;
        let region = case.region;
        let (router, engine) = new_router();
        let connector = serving_connector(&router, &engine, region, name).await;
        let minted = mint_challenges(
            &router,
            region,
            &connector,
            case.challenges,
            case.read,
            case.tags,
            name,
        )
        .await;
        let (challenge, password) = minted[case.read].clone();

        // While the connector is serving the reader hands the very password the create
        // reported back out: the refusals below are about the connector's status, not about
        // the row's challenge.
        let read = get_challenge_password(&router, region, &challenge).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: a read on a {STATUS_ACTIVE} connector's challenge must answer 200, \
             got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["Password"],
            json!(password),
            "case {name:?}: expected the password the create reported ({password:?}), got {:?}",
            read.body
        );

        if case.owner == Owner::Active {
            // Repeating it answers the same password: a read changes nothing.
            let again = get_challenge_password(&router, region, &challenge).await;
            assert_eq!(
                (again.status, &again.body["Password"]),
                (200, &json!(password)),
                "case {name:?}: a repeated read must answer the same password, got {} / {:?}",
                again.status,
                again.body
            );
            continue;
        }

        let deleted = delete_connector(&router, region, &connector).await;
        assert_eq!(
            deleted.status, 202,
            "case {name:?}: expected 202 from DeleteConnector, got {} / {:?}",
            deleted.status, deleted.body
        );

        if case.owner == Owner::Deleting {
            // Pin the teardown window rather than race the service's own transition span.
            harness::hold_pending(&engine, &connector);
            let reported = get_connector(&router, region, &connector).await;
            assert_eq!(
                (reported.status, reported.connector("Status")),
                (200, &json!(STATUS_DELETING)),
                "case {name:?}: the connector must still be reported {STATUS_DELETING}, got {} / \
                 {:?}",
                reported.status,
                reported.body
            );
        } else {
            // The teardown has come due, so the record — challenges included — is gone.
            harness::release_pending(&engine, &connector);
            let gone = get_connector(&router, region, &connector).await;
            assert_eq!(
                gone.status, 404,
                "case {name:?}: once the teardown is due the connector must answer 404, got {} / \
                 {:?}",
                gone.status, gone.body
            );
        }

        // The rule: no password while the connector is not serving — and the same answer every
        // time it is asked for.
        let refused = get_challenge_password(&router, region, &challenge).await;
        assert_withheld(&refused, name, &challenge, &password, "the read");
        let again = get_challenge_password(&router, region, &challenge).await;
        assert_withheld(&again, name, &challenge, &password, "a repeated read");

        // Every challenge of that connector is withheld, not only the row's.
        for (other, other_password) in &minted {
            let read = get_challenge_password(&router, region, other).await;
            assert_withheld(
                &read,
                name,
                other,
                other_password,
                "the read of a sibling challenge",
            );
        }

        // Only the PASSWORD is withheld: while the record is still there, the challenge's
        // metadata keeps being reported, and the refused reads removed nothing.
        let metadata = get_challenge_metadata(&router, region, &challenge).await;
        let expected_metadata = if case.owner == Owner::Deleting { 200 } else { 404 };
        assert_eq!(
            metadata.status, expected_metadata,
            "case {name:?}: expected GetChallengeMetadata to answer {expected_metadata}, got {} / \
             {:?}",
            metadata.status, metadata.body
        );
        if case.owner == Owner::Deleting {
            assert_eq!(
                metadata.body["ChallengeMetadata"]["Arn"],
                json!(challenge),
                "case {name:?}: the reported metadata must be the row's challenge, got {:?}",
                metadata.body
            );
            assert_eq!(
                metadata.body["ChallengeMetadata"]["Password"],
                Value::Null,
                "case {name:?}: ChallengeMetadata models no password, got {:?}",
                metadata.body
            );
        }
    }
}

/// One connector's teardown withholds only ITS challenges' passwords: another connector in the
/// same Region keeps serving its own.
#[tokio::test]
async fn one_connectors_teardown_leaves_every_other_password_readable() {
    let region = REGIONS[0];
    let (router, engine) = new_router();

    let mut resources = Vec::new();
    for which in ["doomed", "survivor"] {
        let connector = serving_connector(&router, &engine, region, which).await;
        let minted = mint_challenges(&router, region, &connector, 1, 0, &[], which).await;
        resources.push((connector, minted[0].clone()));
    }
    let (doomed, (doomed_challenge, doomed_password)) = resources[0].clone();
    let (_survivor, (survivor_challenge, survivor_password)) = resources[1].clone();

    assert_eq!(
        delete_connector(&router, region, &doomed).await.status,
        202,
        "expected 202 from DeleteConnector"
    );
    harness::hold_pending(&engine, &doomed);

    let refused = get_challenge_password(&router, region, &doomed_challenge).await;
    assert_withheld(
        &refused,
        "doomed",
        &doomed_challenge,
        &doomed_password,
        "the read of the torn-down connector's challenge",
    );

    let served = get_challenge_password(&router, region, &survivor_challenge).await;
    assert_eq!(
        (served.status, &served.body["Password"]),
        (200, &json!(survivor_password)),
        "case \"survivor\": another connector's challenge must keep serving its password, got {} / \
         {:?}",
        served.status,
        served.body
    );
}
