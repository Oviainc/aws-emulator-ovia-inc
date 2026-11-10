// SPDX-License-Identifier: CC0-1.0

//! Every reporting operation agrees on which version of a secret is live after a
//! sequence of writes some of which were REFUSED.
//!
//! THE RULE: a refused write creates no secret version, so the staging labels every
//! reporting operation reports stay on the versions the ACCEPTED writes created —
//! `AWSCURRENT` on the last of them, `AWSPREVIOUS` on the one before it ("Secrets
//! Manager automatically moves the staging label AWSCURRENT to the new version.
//! Then it attaches the label AWSPREVIOUS to the version that AWSCURRENT was
//! removed from.", `PutSecretValue`, this service's Smithy model) — and a label no
//! accepted write ever staked is reported by none of them.
//!
//! The suites beside this one pin the refusal itself
//! ([`crate::secret_client_request_token`], as a table over one write) and the
//! version ids the store ends up holding
//! ([`crate::secret_version_ids_are_caller_named`], as an invariant over generated
//! sequences). What this suite adds is the READING surface: a client learns which
//! version is live from `DescribeSecret$VersionIdsToStages`, from
//! `ListSecretVersionIds`' `VersionStages`, and from the version `GetSecretValue`
//! resolves for a staging label — so the three have to tell it the same story, and
//! a refused write has to be invisible to all three. Rotation tooling in particular
//! decides what to move `AWSPENDING`/`AWSCURRENT` onto by reading that map, so a
//! label reported for a version no accepted write created is acted on.
//!
//! Each case is one write sequence, a TABLE row over what the rule generalises: the
//! operation each write uses, the member it carries its value in, whether it names
//! a `ClientRequestToken` (so whether it is served or refused), how many writes
//! there are and in which order the refusals fall — including a sequence long
//! enough that the oldest versions end up deprecated, and one whose writes are all
//! refused, which is the client that reads back its own secret after AWS turned its
//! raw writes down.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_client_request_token::{
    call, emulator, fresh_token, ok, refusal, staging_map, text, value_member,
    MISSING_TOKEN_MESSAGE,
};

/// One write in a case's sequence: the operation, the member it carries its value
/// in, and whether it names a `ClientRequestToken` of its own.
///
/// A write that carries a value and names no token is the one AWS refuses; every
/// other combination is served.
type Write = (&'static str, &'static str, bool);

/// The sequences the rule is checked over. Each row is (case name, the writes made
/// after the secret was created).
const SEQUENCES: &[(&str, &[Write])] = &[
    ("no write after the create", &[]),
    (
        "a token-less put then a token-less update, both refused",
        &[
            ("PutSecretValue", "SecretString", false),
            ("UpdateSecret", "SecretString", false),
        ],
    ),
    (
        "a refused put, then an accepted one",
        &[
            ("PutSecretValue", "SecretBinary", false),
            ("PutSecretValue", "SecretBinary", true),
        ],
    ),
    (
        "an accepted put, then a refused one",
        &[
            ("PutSecretValue", "SecretString", true),
            ("PutSecretValue", "SecretString", false),
        ],
    ),
    (
        "two accepted writes with a refusal between them",
        &[
            ("UpdateSecret", "SecretString", true),
            ("UpdateSecret", "SecretBinary", false),
            ("PutSecretValue", "SecretBinary", true),
        ],
    ),
    (
        "three accepted writes, so the oldest versions are deprecated",
        &[
            ("PutSecretValue", "SecretString", true),
            ("UpdateSecret", "SecretString", true),
            ("PutSecretValue", "SecretBinary", true),
        ],
    ),
    (
        "every refused form, both operations and both value members",
        &[
            ("PutSecretValue", "SecretString", false),
            ("PutSecretValue", "SecretBinary", false),
            ("UpdateSecret", "SecretString", false),
            ("UpdateSecret", "SecretBinary", false),
        ],
    ),
];

/// The two labels a write moves: "Secrets Manager automatically moves the staging
/// label AWSCURRENT to the new version. Then it attaches the label AWSPREVIOUS to
/// the version that AWSCURRENT was removed from." (`PutSecretValue`, this service's
/// Smithy model.)
const STAGE_CURRENT: &str = "AWSCURRENT";
const STAGE_PREVIOUS: &str = "AWSPREVIOUS";

/// One accepted write's trace: the version it staked and the value it stored.
#[derive(Debug, Clone)]
struct StakedVersion {
    version_id: String,
    member: &'static str,
    value: String,
}

/// The labelling the rule leaves on a secret whose versions were staked, in order,
/// by `staked`: `AWSCURRENT` on the last, `AWSPREVIOUS` on the one before it, every
/// older version deprecated and so carrying no label at all.
fn expected_stages(staked: &[StakedVersion], index: usize) -> Option<Vec<String>> {
    match staked.len() - index {
        1 => Some(vec![STAGE_CURRENT.to_string()]),
        2 => Some(vec![STAGE_PREVIOUS.to_string()]),
        _ => None,
    }
}

/// The `Versions` list `ListSecretVersionIds` reports, as (version id, staging
/// labels) in the order the response lists them — the order the versions were
/// created in, per the operation's own paging contract.
async fn listed_versions(
    router: &MultiServiceRouter,
    secret_id: &str,
    include_deprecated: bool,
    case: &str,
) -> Vec<(String, Option<Vec<String>>)> {
    let listed = ok(
        router,
        "ListSecretVersionIds",
        &json!({"SecretId": secret_id, "IncludeDeprecated": include_deprecated}),
        case,
    )
    .await;
    listed
        .get("Versions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .map(|version| {
                    let stages = version.get("VersionStages").and_then(Value::as_array).map(
                        |stages| {
                            stages
                                .iter()
                                .map(|stage| {
                                    stage.as_str().unwrap_or_default().to_string()
                                })
                                .collect()
                        },
                    );
                    (text(version, "VersionId", case), stages)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn refused_writes_are_invisible_to_every_reporting_operation() {
    for (row, (case, writes)) in SEQUENCES.iter().enumerate() {
        let router = emulator();
        // "Name must be a string of ASCII letters, numbers and the characters
        // /_+=.@-" (`CreateSecretRequest$Name`, this service's Smithy model), so the
        // secret is named after the row rather than after the sentence.
        let name = format!("refused-writes/row-{row}");

        // The secret starts from a version its caller named, which is the version
        // the refusals under test must leave current.
        let created_token = fresh_token();
        let created = ok(
            &router,
            "CreateSecret",
            &json!({
                "Name": name.clone(),
                "SecretString": "the-created-value",
                "ClientRequestToken": created_token.clone(),
            }),
            case,
        )
        .await;
        assert_eq!(
            text(&created, "VersionId", case),
            created_token,
            "case {case:?}: expected the create to stake the token it named, got {created}"
        );
        let mut staked = vec![StakedVersion {
            version_id: created_token.clone(),
            member: "SecretString",
            value: "the-created-value".to_string(),
        }];

        for (index, (operation, member, names_a_token)) in writes.iter().enumerate() {
            let step = format!("{case}, step {index}: {operation} carrying {member}");
            let value = format!("the-value-of-step-{index}");
            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name.clone()));
            body.insert((*member).to_string(), value_member(member, &value));
            let token = fresh_token();
            if *names_a_token {
                body.insert("ClientRequestToken".to_string(), json!(token.clone()));
            }
            let (status, parsed) = call(&router, operation, &Value::Object(body)).await;

            if *names_a_token {
                assert_eq!(
                    status, 200,
                    "case {step:?}: expected 200 for a write naming a token, got {status} with \
                     body {parsed}"
                );
                assert_eq!(
                    text(&parsed, "VersionId", &step),
                    token,
                    "case {step:?}: expected the token to become the version id, got {parsed}"
                );
                staked.push(StakedVersion {
                    version_id: token,
                    member,
                    value,
                });
            } else {
                assert_eq!(
                    status, 400,
                    "case {step:?}: expected 400 for a token-less write, got {status} with body \
                     {parsed}"
                );
                let (code, message) = refusal(&parsed);
                assert_eq!(
                    code, "InvalidRequestException",
                    "case {step:?}: expected InvalidRequestException, got {parsed}"
                );
                assert_eq!(
                    message, MISSING_TOKEN_MESSAGE,
                    "case {step:?}: expected the missing-token sentence, got {message:?}"
                );
            }
        }

        // What every reporting operation has to agree on.
        let expected: Vec<(String, Option<Vec<String>>)> = staked
            .iter()
            .enumerate()
            .map(|(index, version)| {
                (
                    version.version_id.clone(),
                    expected_stages(&staked, index),
                )
            })
            .collect();

        // `DescribeSecret` reports the live staging state: "a version with no
        // staging label is deprecated", so a deprecated version is left out of the
        // map rather than reported with an empty list.
        let described = staging_map(&router, &name, case).await;
        let expected_map: Map<String, Value> = expected
            .iter()
            .filter_map(|(version_id, stages)| {
                stages
                    .as_ref()
                    .map(|stages| (version_id.clone(), json!(stages)))
            })
            .collect();
        assert_eq!(
            described, expected_map,
            "case {case:?}: expected DescribeSecret to report the labels of the versions the \
             accepted writes staked, got {described:?}"
        );

        // `ListSecretVersionIds` reports the same labels, for the same versions, in
        // the order they were created — and with `IncludeDeprecated` it reports
        // EVERY version the secret holds, which is where a version a refused write
        // would have minted becomes visible even once it carries no label.
        let all_versions = listed_versions(&router, &name, true, case).await;
        assert_eq!(
            all_versions, expected,
            "case {case:?}: expected ListSecretVersionIds to hold exactly the versions the \
             accepted writes staked, got {all_versions:?}"
        );
        let labelled = listed_versions(&router, &name, false, case).await;
        let expected_labelled: Vec<(String, Option<Vec<String>>)> = expected
            .iter()
            .filter(|(_, stages)| stages.is_some())
            .cloned()
            .collect();
        assert_eq!(
            labelled, expected_labelled,
            "case {case:?}: expected the default listing to omit the deprecated versions, got \
             {labelled:?}"
        );

        // And the read resolves those labels to those versions: the value the last
        // ACCEPTED write stored for `AWSCURRENT`, the one before it for
        // `AWSPREVIOUS`, and a refusal naming the label when no accepted write ever
        // staked one.
        let current = staked.last().expect("the created version is always staked");
        for selector in [json!({"SecretId": name.clone()}), json!({"SecretId": name.clone(), "VersionStage": STAGE_CURRENT})] {
            let read = ok(&router, "GetSecretValue", &selector, case).await;
            assert_eq!(
                text(&read, "VersionId", case),
                current.version_id,
                "case {case:?}: expected {selector} to resolve the last version an accepted \
                 write staked, got {read}"
            );
            assert_eq!(
                read.get(current.member),
                Some(&value_member(current.member, &current.value)),
                "case {case:?}: expected {selector} to answer with the value that write stored, \
                 got {read}"
            );
        }

        let previous = staked.len().checked_sub(2).map(|index| &staked[index]);
        let (status, parsed) = call(
            &router,
            "GetSecretValue",
            &json!({"SecretId": name.clone(), "VersionStage": STAGE_PREVIOUS}),
        )
        .await;
        match previous {
            Some(previous) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected AWSPREVIOUS to resolve, got {status} with body \
                     {parsed}"
                );
                assert_eq!(
                    text(&parsed, "VersionId", case),
                    previous.version_id,
                    "case {case:?}: expected AWSPREVIOUS on the version before the current one, \
                     got {parsed}"
                );
            }
            // Only one version was ever staked, so no write has demoted anything:
            // the label names no version, which the read answers with
            // `ResourceNotFoundException` naming the label it could not resolve.
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected AWSPREVIOUS to resolve nowhere, got {status} with \
                     body {parsed}"
                );
                let (code, message) = refusal(&parsed);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {parsed}"
                );
                assert_eq!(
                    message,
                    format!(
                        "Secrets Manager can't find the specified secret value for staging \
                         label: {STAGE_PREVIOUS}"
                    ),
                    "case {case:?}: expected the staging-label sentence, got {message:?}"
                );
            }
        }
    }
}
