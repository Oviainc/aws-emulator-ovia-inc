// SPDX-License-Identifier: CC0-1.0

//! The diagnostic `CreateTrust` refuses a remote domain a directory already trusts with.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.CreateTrust` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the directory-lifecycle suite's,
//! reached as `crate::directory_lifecycle::<item>`).
//!
//! RULE: Directory Service states this refusal as the PAIRING that already exists — the
//! directory the trust hangs off and the remote domain it points at, both as the request
//! named them, closed by the request-id suffix every refusal of this service carries. It
//! names nothing of the stored trust itself (not its id, not its state, not the spelling
//! the first create used), which is what keeps the diagnostic a function of the request.
//!
//! The table spans what the template interpolates: two directories, several remote domains
//! and every spelling of one domain the lookup folds together (case, and the trailing root
//! dot the modelled `RemoteDomainName` pattern admits).

use serde_json::json;

use crate::directory_lifecycle::{call, ok, service, string, PAST_ONE_SPAN};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The trust password the creates below use: the modelled `TrustPassword` admits 1..128
/// characters of letters, digits, punctuation and spaces.
const TRUST_PASSWORD: &str = "Sim2Real-Trust-Pw1!";

/// The direction every trust below is created with: a value of the modelled
/// `TrustDirection` enum.
const TRUST_DIRECTION: &str = "Two-Way";

/// RULE: the duplicate-remote-domain refusal states the directory and the remote domain the
/// REQUEST named, and nothing of the stored trust.
#[tokio::test]
async fn the_duplicate_remote_domain_refusal_states_the_requested_pairing() {
    let router = service();

    let first = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("corp.example.com"),
        "the first directory",
    )
    .await;
    let second = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("ops.example.com"),
        "the second directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // (the directory, the domain the first create spells, the spellings the second create
    // asks for). Every spelling names the same DNS domain, so each is a duplicate.
    let rows = [
        (
            &first,
            "remote.example.com",
            vec!["remote.example.com", "REMOTE.EXAMPLE.COM", "remote.example.com."],
        ),
        (
            &second,
            "partner-dept.example.net",
            vec!["partner-dept.example.net", "Partner-Dept.Example.Net."],
        ),
    ];

    for (directory, domain, duplicates) in rows {
        let case = format!("{directory} trusting {domain}");
        let created = ok(
            &router,
            "CreateTrust",
            json!({
                "DirectoryId": directory,
                "RemoteDomainName": domain,
                "TrustPassword": TRUST_PASSWORD,
                "TrustDirection": TRUST_DIRECTION,
            }),
            &case,
        )
        .await;
        let trust_id = string(&created, "TrustId", &case);

        for spelling in duplicates {
            let case = format!("{case}, again as {spelling:?}");
            let (status, parsed) = call(
                &router,
                "CreateTrust",
                json!({
                    "DirectoryId": directory,
                    "RemoteDomainName": spelling,
                    "TrustPassword": TRUST_PASSWORD,
                    "TrustDirection": TRUST_DIRECTION,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} with {parsed}"
            );
            let wire_type = parsed["__type"].as_str().unwrap_or_default();
            assert!(
                wire_type.contains("EntityAlreadyExistsException"),
                "case {case:?}: expected EntityAlreadyExistsException, got {parsed}"
            );
            // The whole message: the pairing the request named, then the request-id
            // suffix every refusal of this service closes with, carrying the id the
            // answer reports in its own member.
            let request_id = parsed["RequestId"].as_str().unwrap_or_default();
            let message = parsed["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "Trust already exists for directory: {directory} and remote domain: \
                     {spelling}. : RequestId: {request_id}"
                ),
                "case {case:?}: unexpected diagnostic"
            );
            // Nothing of the stored trust leaks into it.
            assert!(
                !message.contains(&trust_id),
                "case {case:?}: expected the refusal to disclose no trust id, got {message:?}"
            );
        }
    }
}
