// SPDX-License-Identifier: CC0-1.0

//! The one prerequisite single sign-on has — the directory's ACCESS URL — driven end to
//! end over HTTP, with `DescribeDirectories` as the reader that proves what was
//! persisted.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the
//! state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test, asserted over a TABLE of alias forms rather than one recorded
//! call: `EnableSso` succeeds only for a directory that HAS an access URL, which exists
//! only once an alias has been assigned to it. Until then the directory's own reader
//! reports the directory id as its `Alias` and that id's host as its `AccessUrl`, and the
//! enable is refused with the modelled `ClientException` (HTTP 400) naming the directory
//! the request asked for and recording nothing. Once `CreateAlias` has assigned any alias
//! the model admits, the reader reports that alias and the host derived from it, and the
//! IDENTICAL enable answers 200 with an empty body and the reader then reports
//! `SsoEnabled` as true.
//!
//! The rows differ along what the rule generalises over — the FORM of the alias that
//! supplies the access URL: length (each published bound), alphabet, case, hyphen
//! placement, and a leading `d` that is not the prefix the pattern forbids. The second
//! case takes the same rule from the other side: an alias the MODEL refuses assigns none,
//! so the access URL still does not exist and the enable is still refused — over a table
//! of refused forms including one past each published length bound.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, service, string, PAST_ONE_SPAN,
};

/// The DNS suffix an access URL is built on, from the `AccessUrl` member's own
/// documentation: the alias "is used to construct the access URL for the directory, such
/// as `http://<alias>.awsapps.com`"
/// (`smithy_query(query="doc", shape="DirectoryDescription", member="AccessUrl")`). The
/// member reports the HOST alone.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// An alias at `AliasName`'s published maximum length of 62.
///
/// The bound is the model's: `CreateAlias$Alias` is `@length 1..62`
/// (`smithy_query(query="operation", operation="CreateAlias")`).
const LONGEST_ALIAS: &str = "sso-alias-at-the-published-maximum-length-of-62-characters-abc";

/// Aliases the model admits, as `(alias, class)`.
///
/// Every row satisfies `AliasName`'s published constraints — `@length 1..62` and
/// `@pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$`
/// (`smithy_query(query="operation", operation="CreateAlias")`) — and the rows differ
/// along what that pattern and that length admit: both published length bounds, each
/// alphabet the pattern allows, letter case, hyphen placement, and a leading `d` that is
/// NOT the `d-` prefix the pattern's lookahead forbids.
const ADMITTED_ALIASES: [(&str, &str); 6] = [
    ("a", "a single letter, the published minimum length of 1"),
    ("7", "a single digit"),
    ("sso-access-url", "hyphen-separated labels"),
    ("MixedCase123", "mixed-case letters and digits"),
    (
        "dsso-1",
        "a leading 'd' that is not the forbidden 'd-' prefix",
    ),
    (
        LONGEST_ALIAS,
        "the published maximum length of 62 characters",
    ),
];

/// The `Message` of a refusal body, whichever casing the envelope carries it in: a
/// modelled error of this service reports `Message`, the boundary's `ValidationException`
/// reports `message`.
fn message(parsed: &serde_json::Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The diagnostic this service answers a missing access URL with: the sentence, then the
/// request-id suffix it appends to every refusal it words. The directory id is the
/// request's, and the request id is the one the SAME refusal reports.
fn alias_required(directory_id: &str, reported_request_id: &str) -> String {
    format!(
        "An alias is required before enabling SSO. DomainId={directory_id} : RequestId: \
         {reported_request_id}"
    )
}

/// What the reader reports about a directory's access URL and single sign-on, as
/// `(Alias, AccessUrl, SsoEnabled)`.
async fn projection(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> (String, String, bool) {
    let described = describe_one(router, directory_id, case).await;
    let sso_enabled = described["SsoEnabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean SsoEnabled, got {described}"));
    (
        string(&described, "Alias", case),
        string(&described, "AccessUrl", case),
        sso_enabled,
    )
}

/// Assert the directory has NO access URL of its own yet: its `Alias` is the directory id
/// and its `AccessUrl` is that id's host, which is how a caller observes the prerequisite
/// as unmet, and single sign-on is off.
async fn assert_no_access_url(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let (alias, access_url, sso_enabled) = projection(router, directory_id, case).await;
    assert_eq!(
        alias, directory_id,
        "case {case:?}: expected Alias to stand in as the directory id, got {alias:?}"
    );
    assert_eq!(
        access_url,
        format!("{directory_id}{ACCESS_URL_SUFFIX}"),
        "case {case:?}: expected the access URL derived from the standing-in alias, got \
         {access_url:?}"
    );
    assert!(
        !sso_enabled,
        "case {case:?}: expected single sign-on off while the access URL does not exist"
    );
}

/// `EnableSso` must be refused for a missing access URL, recording nothing.
async fn assert_refused_for_the_access_url(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) {
    let (status, parsed) = call(router, "EnableSso", json!({ "DirectoryId": directory_id })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected EnableSso to answer 400 with no access URL, got {status} \
         with {parsed}"
    );
    assert_eq!(
        parsed["__type"],
        json!("ClientException"),
        "case {case:?}: expected the modelled client refusal, got {parsed}"
    );
    let reported = string(&parsed, "RequestId", case);
    assert_eq!(
        message(&parsed, case),
        alias_required(directory_id, &reported),
        "case {case:?}: expected the missing-access-URL sentence, got {parsed}"
    );
    assert_no_access_url(router, directory_id, case).await;
}

/// Create one directory per row and wait out the transitions its create owes, so every
/// row's directory is in service before an alias is assigned to it.
async fn directories_for(router: &MultiServiceRouter, rows: &[(String, String)]) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, (_, case)) in rows.iter().enumerate() {
        let id = create_directory(
            router,
            minimal_create(&format!("row{index}.example.com")),
            case,
        )
        .await;
        ids.push(id);
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ids
}

#[tokio::test]
async fn single_sign_on_waits_for_the_access_url_an_alias_supplies() {
    let router = service();
    assert_eq!(
        LONGEST_ALIAS.len(),
        62,
        "the maximum-length row must sit exactly on the published bound of 62, got {}",
        LONGEST_ALIAS.len()
    );

    let rows: Vec<(String, String)> = ADMITTED_ALIASES
        .iter()
        .map(|(alias, class)| ((*alias).to_string(), (*class).to_string()))
        .collect();
    let ids = directories_for(&router, &rows).await;

    for ((alias, case), id) in rows.iter().zip(&ids) {
        // No alias yet, so no access URL, so no enable.
        assert_no_access_url(&router, id, case).await;
        assert_refused_for_the_access_url(&router, id, case).await;

        // The one missing prerequisite, supplied. The create echoes only what it took.
        let assigned = ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": alias }),
            case,
        )
        .await;
        assert_eq!(
            (
                string(&assigned, "DirectoryId", case),
                string(&assigned, "Alias", case)
            ),
            (id.clone(), alias.clone()),
            "case {case:?}: expected CreateAlias to echo the directory and the alias it \
             took, got {assigned}"
        );

        // The access URL now exists and is derived from that alias — and assigning it
        // did not turn single sign-on on by itself.
        let (reported_alias, access_url, sso_enabled) = projection(&router, id, case).await;
        assert_eq!(
            (reported_alias.as_str(), access_url.as_str(), sso_enabled),
            (
                alias.as_str(),
                format!("{alias}{ACCESS_URL_SUFFIX}").as_str(),
                false
            ),
            "case {case:?}: expected the alias and its access URL with single sign-on \
             still off, got {reported_alias:?}/{access_url:?}/{sso_enabled}"
        );

        // The IDENTICAL enable is now admitted, with the modelled empty body.
        let answer = ok(&router, "EnableSso", json!({ "DirectoryId": id }), case).await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: EnableSso has no modelled output members, got {answer}"
        );

        let (reported_alias, access_url, sso_enabled) = projection(&router, id, case).await;
        assert_eq!(
            (reported_alias.as_str(), access_url.as_str(), sso_enabled),
            (
                alias.as_str(),
                format!("{alias}{ACCESS_URL_SUFFIX}").as_str(),
                true
            ),
            "case {case:?}: expected single sign-on on with the access URL unchanged, got \
             {reported_alias:?}/{access_url:?}/{sso_enabled}"
        );
    }

    // Each row's flag is its own: reading them all back again reports every one of them
    // on, and none of the enables above reached another row's directory.
    for ((alias, case), id) in rows.iter().zip(&ids) {
        let (reported_alias, _, sso_enabled) = projection(&router, id, case).await;
        assert_eq!(
            (reported_alias.as_str(), sso_enabled),
            (alias.as_str(), true),
            "case {case:?}: expected a repeated read to report the same alias and flag, \
             got {reported_alias:?}/{sso_enabled}"
        );
    }
}

#[tokio::test]
async fn an_alias_the_model_refuses_leaves_the_access_url_missing() {
    let router = service();

    // Forms the model refuses, as `(alias, class)`: one past each published length bound
    // of `@length 1..62`, both spellings of the prefix the pattern's `(?!D-|d-)`
    // lookahead forbids, a character outside its alphabet, and a leading hyphen its
    // first group forbids
    // (`smithy_query(query="operation", operation="CreateAlias")`).
    let rows: Vec<(String, String)> = vec![
        (
            format!("{LONGEST_ALIAS}x"),
            "one character past the published maximum length of 62".to_string(),
        ),
        (
            String::new(),
            "empty, one character below the published minimum length of 1".to_string(),
        ),
        (
            "d-0a1b2c3d4e".to_string(),
            "the forbidden lower-case directory-id prefix".to_string(),
        ),
        (
            "D-0A1B2C3D4E".to_string(),
            "the forbidden upper-case directory-id prefix".to_string(),
        ),
        (
            "sso_underscore".to_string(),
            "a character outside the pattern's alphabet".to_string(),
        ),
        (
            "-sso".to_string(),
            "a leading hyphen the pattern's first group forbids".to_string(),
        ),
    ];
    let ids = directories_for(&router, &rows).await;

    for (index, ((alias, case), id)) in rows.iter().zip(&ids).enumerate() {
        let (status, parsed) = call(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": alias }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the model refuses the alias {alias:?}, got {status} with \
             {parsed}"
        );

        // Nothing was assigned, so the access URL still does not exist and the enable is
        // refused for exactly that — not for the alias the caller sent.
        assert_no_access_url(&router, id, case).await;
        assert_refused_for_the_access_url(&router, id, case).await;

        // The directory is not poisoned: an alias the model admits still assigns, and the
        // identical enable is then admitted.
        let recovery = format!("sso-recovered-{index}");
        ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": &recovery }),
            case,
        )
        .await;
        ok(&router, "EnableSso", json!({ "DirectoryId": id }), case).await;
        let (reported_alias, access_url, sso_enabled) = projection(&router, id, case).await;
        assert_eq!(
            (reported_alias.as_str(), access_url.as_str(), sso_enabled),
            (
                recovery.as_str(),
                format!("{recovery}{ACCESS_URL_SUFFIX}").as_str(),
                true
            ),
            "case {case:?}: expected the admitted alias, its access URL and single \
             sign-on on, got {reported_alias:?}/{access_url:?}/{sso_enabled}"
        );
    }
}
