// SPDX-License-Identifier: CC0-1.0

//! RE-ALIASING a directory: a directory that already carries an alias can never be given
//! another one, and that is a DIFFERENT failure from an alias another directory reserved.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body
//! (`crate::directory_lifecycle`'s harness), so nothing here reaches into the state struct
//! and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test — `CreateAlias`' own warning is "after an alias has been created, it
//! cannot be deleted or reused", so each collision is refused with the class that tells a
//! caller what to do about it:
//!
//! * a directory that already carries an alias is refused with `InvalidParameterException`
//!   stating the fixed sentence of that rule, which quotes neither the directory nor either
//!   alias: no value the caller sends can make the call succeed;
//! * an alias ANOTHER directory (or a deleted one) reserved is refused with
//!   `EntityAlreadyExistsException` naming the value that is taken, which the caller can
//!   retry with another one;
//! * either refusal stores nothing: the directory keeps the alias it had and the
//!   `AccessUrl` derived from it.
//!
//! The rows differ along what the rule generalises over: the directory's type (Simple AD
//! and Managed Microsoft AD), the form of the alias already held and of the one asked for
//! (short, long, hyphenated, digits, both published length bounds), and how many further
//! attempts are made on one directory.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::ALIAS_ALREADY_ASSIGNED;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, service, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// What this service's refusal envelope appends to every diagnostic it words: the call's own
/// correlation handle, whose value is per call.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// The sentence Directory Service states this rule in, spelled out here rather than taken
/// from the service: the shared constant is what the other rows assert through, so this is
/// the one place the WORDING itself is pinned — a constant whose text drifted would still
/// satisfy them, and would fail here.
const AWS_REALIAS_SENTENCE: &str = "The directory in the request already has an alias. That \
                                    alias must be deleted before a new alias can be created.";

/// The DNS suffix an access URL is built on, from `AccessUrl`'s own documentation.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// One row: the alias a directory is given, and the ones later asked for on it.
struct ReAliasCase {
    case: &'static str,
    /// Whether the subject is a Managed Microsoft AD directory rather than a Simple AD one.
    managed: bool,
    /// The alias the directory really gets.
    held: &'static str,
    /// The further aliases asked for on that same directory, each refused.
    refused_attempts: &'static [&'static str],
}

/// The rows: every form `AliasName` admits, held and asked for, across both directory types
/// this service creates in a VPC. The maximum row holds a 62-character alias — the upper
/// bound of the modelled `@length` on `AliasName` — and asks for a one-character one, the
/// lower bound.
const CASES: [ReAliasCase; 5] = [
    ReAliasCase {
        case: "a Simple AD directory, plain aliases",
        managed: false,
        held: "firstalias",
        refused_attempts: &["secondalias", "thirdalias"],
    },
    ReAliasCase {
        case: "a Managed Microsoft AD directory, plain aliases",
        managed: true,
        held: "managedalias",
        refused_attempts: &["anotheralias"],
    },
    ReAliasCase {
        case: "a hyphenated alias held, a digits-only one asked for",
        managed: true,
        held: "corp-ad-01",
        refused_attempts: &["20261108", "corp-ad-02"],
    },
    ReAliasCase {
        case: "the longest alias held, the shortest asked for",
        managed: false,
        held: "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgg",
        refused_attempts: &["a"],
    },
    ReAliasCase {
        case: "a single-character alias held, a longer one asked for",
        managed: false,
        held: "z",
        refused_attempts: &["zzzzzzzzzzzz"],
    },
];

/// `CreateAlias`, asserting it was accepted and reported the pair it stored.
async fn assign(router: &MultiServiceRouter, directory_id: &str, alias: &str, case: &str) {
    let answer = ok(
        router,
        "CreateAlias",
        json!({ "DirectoryId": directory_id, "Alias": alias }),
        case,
    )
    .await;
    assert_eq!(
        (answer["DirectoryId"].clone(), answer["Alias"].clone()),
        (json!(directory_id), json!(alias)),
        "case {case:?}: expected CreateAlias to report the pair it stored, got {answer}"
    );
}

/// One refused `CreateAlias`, as the HTTP status, the wire error code and the body.
async fn attempt(
    router: &MultiServiceRouter,
    directory_id: &str,
    alias: &str,
) -> (u16, String, Value) {
    let (status, parsed) = call(
        router,
        "CreateAlias",
        json!({ "DirectoryId": directory_id, "Alias": alias }),
    )
    .await;
    let code = parsed["__type"].as_str().unwrap_or_default().to_string();
    (status, code, parsed)
}

/// RULE: a directory that already carries an alias is refused another with 400
/// `InvalidParameterException` stating that rule's fixed sentence, and the refusal changes
/// nothing the directory reports.
#[tokio::test]
async fn a_directory_that_already_has_an_alias_is_refused_another() {
    let router = service();
    assert_eq!(
        ALIAS_ALREADY_ASSIGNED, AWS_REALIAS_SENTENCE,
        "the sentence this service states must be AWS's own for this refusal"
    );

    for (index, row) in CASES.iter().enumerate() {
        let name = format!("realias{index}.example.com");
        let directory_id = if row.managed {
            create_microsoft_ad(&router, minimal_microsoft_ad(&name), row.case).await
        } else {
            create_directory(&router, minimal_create(&name), row.case).await
        };
        // An alias is assigned to a directory that is IN SERVICE, so each row waits for the
        // stage transition its create owes before the first alias is given.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        assign(&router, &directory_id, row.held, row.case).await;

        for alias in row.refused_attempts {
            let case = format!("{} — {alias:?} after {:?}", row.case, row.held);
            let (status, code, parsed) = attempt(&router, &directory_id, alias).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the re-alias to be refused, got {status} with {parsed}"
            );
            assert!(
                code.contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {parsed}"
            );
            let message = parsed["Message"].as_str().unwrap_or_default();
            let request_id = parsed["RequestId"].as_str().unwrap_or_default();
            assert!(
                !request_id.is_empty(),
                "case {case:?}: expected the refusal to report a RequestId, got {parsed}"
            );
            assert_eq!(
                message,
                format!("{ALIAS_ALREADY_ASSIGNED}{REQUEST_ID_SEPARATOR}{request_id}"),
                "case {case:?}: expected the fixed re-alias sentence, got {message:?}"
            );
            // Nothing of the request is quoted: neither alias and not the directory, so a
            // client matching the sentence matches it for every directory. (A value short
            // enough to occur inside ordinary prose is left out of this check — what the
            // sentence IS has already been asserted above.)
            for quoted in [row.held, alias, directory_id.as_str()] {
                if quoted.len() < 4 {
                    continue;
                }
                assert!(
                    !message.contains(quoted),
                    "case {case:?}: expected the sentence to quote nothing of the request, \
                     got {message:?} carrying {quoted:?}"
                );
            }
        }

        // The refusals stored nothing: the directory still reports the alias it was given
        // and the access URL derived from it.
        let described = describe_one(&router, &directory_id, row.case).await;
        assert_eq!(
            (described["Alias"].clone(), described["AccessUrl"].clone()),
            (
                json!(row.held),
                json!(format!("{held}{ACCESS_URL_SUFFIX}", held = row.held))
            ),
            "case {case:?}: expected the first alias and its access URL to stand, got \
             {described}",
            case = row.case
        );
    }
}

/// RULE: an alias another directory reserved is the OTHER failure — the value is taken, so
/// it is `EntityAlreadyExistsException` naming it, and a directory with no alias of its own
/// is never refused the re-alias sentence.
#[tokio::test]
async fn an_alias_another_directory_reserved_is_a_different_refusal() {
    let router = service();
    let case = "a reserved alias";

    let holder = create_directory(&router, minimal_create("holder.example.com"), case).await;
    // A reservation outlives the directory that made it, so a deleted directory's alias is
    // taken too. Both directories are given their alias once they are in service, which is
    // when this service assigns one.
    let doomed = create_directory(&router, minimal_create("doomed.example.com"), case).await;
    let fresh = create_directory(&router, minimal_create("fresh.example.com"), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    assign(&router, &holder, "takenalias", case).await;
    assign(&router, &doomed, "goneaswell", case).await;
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        case,
    )
    .await;

    // A directory with NO alias of its own: every row below is refused for the value it
    // named rather than for what the directory is.
    for alias in ["takenalias", "goneaswell"] {
        let row = format!("{case}: {alias:?}");
        let (status, code, parsed) = attempt(&router, &fresh, alias).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected the taken alias to be refused, got {status} with {parsed}"
        );
        assert!(
            code.contains("EntityAlreadyExistsException"),
            "case {row:?}: expected EntityAlreadyExistsException, got {parsed}"
        );
        let message = parsed["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(alias) && !message.contains(ALIAS_ALREADY_ASSIGNED),
            "case {row:?}: expected the refusal to name the taken value rather than the \
             re-alias rule, got {message:?}"
        );
    }

    // The refused rows left the fresh directory without an alias, so a value nobody
    // reserved is still assignable — and only then is a further one the invalid parameter.
    let described = describe_one(&router, &fresh, case).await;
    assert_eq!(
        described["Alias"],
        json!(&fresh),
        "case {case:?}: expected the refused attempts to leave the default alias, got \
         {described}"
    );
    assign(&router, &fresh, "freealias", case).await;
    let (status, code, parsed) = attempt(&router, &fresh, "onemorealias").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the second alias to be refused, got {status} with {parsed}"
    );
    assert!(
        code.contains("InvalidParameterException"),
        "case {case:?}: expected InvalidParameterException once the directory holds an \
         alias, got {parsed}"
    );
}
