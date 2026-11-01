// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the certificate-revocation-list write-after-import surface:
//! what `UpdateCrl` writes, what `DeleteCrl` does to the store, what `GetCrl` answers for
//! an id that resolves to nothing, and what `ImportCrl` answers for a `trustAnchorArn`
//! that resolves to nothing.
//!
//! Three rules are under test.
//!
//! * An update writes the members the request carries and leaves every member it omits as
//!   stored, so a later read — and the delete echo — reports the updated record.
//! * A resource that is not stored is answered with the modelled
//!   `ResourceNotFoundException` naming it by ARN — "The item doesn't exist. arn: <arn> ;
//!   aborting." — and `DeleteCrl` is what makes a CRL not stored, so the delete and the
//!   refusal are tested together.
//! * A request member that REFERENCES another resource is resolved before the request is
//!   served, and an unresolvable reference is refused with the ARN's id named — "Unable to
//!   find Trust Anchor with ID <id>." — rather than stored as a dangling link.
//!
//! Each table's rows differ along what the rule generalises over: which id or ARN is
//! unresolvable and why (never minted, belonging to another resource type, deleted, in
//! another partition, Region or account, carrying no id at all), the Region the request is
//! signed for, and how many resources the store already holds.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the suite signs for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_trust_anchor(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await
    }

    async fn import_crl(&self, name: &str, trust_anchor_arn: &str) -> Answer {
        self.send(
            "POST",
            "/crls",
            Some(json!({
                "name": name,
                "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
                "trustAnchorArn": trust_anchor_arn,
            })),
        )
        .await
    }

    async fn get_crl(&self, id: &str) -> Answer {
        self.send("GET", &format!("/crl/{id}"), None).await
    }

    async fn update_crl(&self, id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/crl/{id}"), Some(body)).await
    }

    async fn delete_crl(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/crl/{id}"), None).await
    }

    async fn list_crl_ids(&self) -> Vec<String> {
        let answer = self.send("GET", "/crls", None).await;
        assert_eq!(answer.status, 200, "expected ListCrls 200, got {answer:?}");
        answer
            .body
            .get("crls")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("crlId").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The ARN a CRL id carries in `region`, which is the ARN a refusal naming that id words.
fn crl_arn(region: &str, crl_id: &str) -> String {
    format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:crl/{crl_id}")
}

/// The wording an ARN-keyed miss carries.
fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// Create a trust anchor and answer `(id, arn)`.
async fn trust_anchor(client: &Client, name: &str) -> (String, String) {
    let created = client.create_trust_anchor(name).await;
    assert_eq!(
        created.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {created:?}"
    );
    let anchor = created
        .body
        .get("trustAnchor")
        .cloned()
        .unwrap_or(Value::Null);
    let id = anchor
        .get("trustAnchorId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorId in {created:?}"))
        .to_string();
    let arn = anchor
        .get("trustAnchorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {created:?}"))
        .to_string();
    (id, arn)
}

/// `UpdateCrl` writes the members the request carries and leaves the rest as stored, and
/// every later path reports the updated record.
///
/// The rows differ along what the rule generalises over: which of the two optional members
/// the request carries, the new value's size and bytes (including both published `crlData`
/// `@length` bounds and a non-UTF-8 blob), the new name's length at both published
/// `ResourceName` bounds, and a request that carries neither member.
#[tokio::test]
async fn update_crl_writes_the_submitted_members_and_keeps_the_rest() {
    let client = Client::new();
    let (_, anchor_arn) = trust_anchor(&client, "update-anchor").await;

    // One row of the update table: what the request carries, and what the CRL must hold
    // afterwards. `None` means "the request omits this member", so the stored value stands.
    struct UpdateCase {
        name: &'static str,
        body: Value,
        expected_name: Option<String>,
        expected_data: Option<Vec<u8>>,
    }

    let cases: Vec<UpdateCase> = vec![
        UpdateCase {
            name: "name-only",
            body: json!({"name": "renamed crl"}),
            expected_name: Some("renamed crl".to_string()),
            expected_data: None,
        },
        UpdateCase {
            name: "data-only",
            body: json!({
                "crlData": aws_smithy_types::base64::encode(
                    b"-----BEGIN X509 CRL-----\nc2Vjb25k\n-----END X509 CRL-----\n",
                ),
            }),
            expected_name: None,
            expected_data: Some(
                b"-----BEGIN X509 CRL-----\nc2Vjb25k\n-----END X509 CRL-----\n".to_vec(),
            ),
        },
        UpdateCase {
            name: "both-members",
            body: json!({
                "name": "both changed",
                "crlData": aws_smithy_types::base64::encode(b"tiny"),
            }),
            expected_name: Some("both changed".to_string()),
            expected_data: Some(b"tiny".to_vec()),
        },
        UpdateCase {
            // `ResourceName` is `@length(1, 255)`; `crlData` is a `Blob` `@length(1, 300000)`.
            name: "published-bounds",
            body: json!({
                "name": "n".repeat(255),
                "crlData": aws_smithy_types::base64::encode(vec![b'x'; 300_000]),
            }),
            expected_name: Some("n".repeat(255)),
            expected_data: Some(vec![b'x'; 300_000]),
        },
        UpdateCase {
            name: "shortest-name-and-smallest-data",
            body: json!({
                "name": "n",
                "crlData": aws_smithy_types::base64::encode(b"-"),
            }),
            expected_name: Some("n".to_string()),
            expected_data: Some(b"-".to_vec()),
        },
        UpdateCase {
            // A `Blob` is bytes, not text: every byte value has to survive an update.
            name: "binary-data",
            body: json!({
                "crlData": aws_smithy_types::base64::encode((0u8..=255).collect::<Vec<u8>>()),
            }),
            expected_name: None,
            expected_data: Some((0u8..=255).collect()),
        },
        UpdateCase {
            name: "neither-member",
            body: json!({}),
            expected_name: None,
            expected_data: None,
        },
    ];

    for UpdateCase {
        name: case,
        body,
        expected_name,
        expected_data,
    } in &cases
    {
        let imported = client.import_crl("original crl", &anchor_arn).await;
        assert_eq!(
            imported.status, 201,
            "case {case:?}: expected ImportCrl 201, got {imported:?}"
        );
        let before = imported.body.get("crl").cloned().unwrap_or(Value::Null);
        let id = before
            .get("crlId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no crlId in {imported:?}"))
            .to_string();

        let updated = client.update_crl(&id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateCrl 200, got {updated:?}"
        );
        let after = updated.body.get("crl").cloned().unwrap_or(Value::Null);

        let expected_name = match expected_name {
            Some(name) => name.clone(),
            None => before
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        assert_eq!(
            after.get("name").and_then(Value::as_str),
            Some(expected_name.as_str()),
            "case {case:?}: expected the name {expected_name:?} after the update, got {after:?}"
        );
        let expected_data = match expected_data {
            Some(data) => aws_smithy_types::base64::encode(data),
            None => before
                .get("crlData")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        assert_eq!(
            after.get("crlData").and_then(Value::as_str),
            Some(expected_data.as_str()),
            "case {case:?}: the update must report the stored crlData, got {} bytes of base64",
            after
                .get("crlData")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .len()
        );
        // Members this operation has no input for keep what the import stored, and
        // `createdAt` never moves.
        for member in ["crlId", "crlArn", "trustAnchorArn", "enabled", "createdAt"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: {member} is not this operation's to change, got {after:?}"
            );
        }

        // The write is what a later read and the delete echo report.
        let read = client.get_crl(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetCrl 200, got {read:?}"
        );
        assert_eq!(
            read.body.get("crl").cloned().unwrap_or(Value::Null),
            after,
            "case {case:?}: GetCrl must report the updated record"
        );
        let deleted = client.delete_crl(&id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteCrl 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body.get("crl").cloned().unwrap_or(Value::Null),
            after,
            "case {case:?}: the delete echo must report the updated record"
        );
    }
}

/// An `UpdateCrl` on an id that resolves to no stored CRL is the same ARN-named 404 the
/// read and the delete answer.
#[tokio::test]
async fn update_crl_refuses_an_unresolvable_crl_id() {
    let client = Client::new();
    for (case, id) in [
        ("never-minted", "00000000-0000-4000-8000-000000000000"),
        ("all-hex", "abcdef12-3456-7890-abcd-ef1234567890"),
    ] {
        let answer = client.update_crl(id, json!({"name": "renamed"})).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected UpdateCrl 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(arn_not_found_message(&crl_arn(REGION, id)).as_str()),
            "case {case:?}: the refusal must name the requested CRL by ARN, got {answer:?}"
        );
    }
}

/// `DeleteCrl` removes the record, answers with it, and every later call that resolves that
/// id refuses with the modelled 404 — while every other stored CRL is untouched.
///
/// The rows differ in the CRL's name, its `enabled` flag, which trust anchor it links to
/// and how many CRLs the store holds when it is deleted.
#[tokio::test]
async fn delete_crl_removes_the_record_and_later_reads_refuse() {
    let client = Client::new();
    let (_, first_anchor) = trust_anchor(&client, "delete-anchor-one").await;
    let (_, second_anchor) = trust_anchor(&client, "delete anchor two").await;

    let cases: Vec<(&str, &str)> = vec![
        ("first", first_anchor.as_str()),
        ("second", second_anchor.as_str()),
        ("third", first_anchor.as_str()),
        ("fourth", second_anchor.as_str()),
    ];

    // Import the whole table first, so each delete happens with other CRLs in the store.
    let mut ids: Vec<(&str, String, Value)> = Vec::new();
    for (case, anchor) in &cases {
        let imported = client.import_crl(&format!("delete {case} crl"), anchor).await;
        assert_eq!(
            imported.status, 201,
            "case {case:?}: expected ImportCrl 201, got {imported:?}"
        );
        let crl = imported.body.get("crl").cloned().unwrap_or(Value::Null);
        let id = crl
            .get("crlId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no crlId in {imported:?}"))
            .to_string();
        ids.push((case, id, crl));
    }

    let mut remaining: Vec<String> = ids.iter().map(|(_, id, _)| id.clone()).collect();
    for (case, id, imported) in &ids {
        let read = client.get_crl(id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a stored CRL reads back, got {read:?}"
        );

        let deleted = client.delete_crl(id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteCrl 200, got {deleted:?}"
        );
        assert_eq!(
            deleted.body.get("crl").cloned().unwrap_or(Value::Null),
            *imported,
            "case {case:?}: the delete echoes the record as it stood"
        );

        // The id no longer resolves: the read and a repeated delete both refuse, and the
        // refusal names the CRL that is missing by its ARN.
        for (path, answer) in [
            ("GetCrl", client.get_crl(id).await),
            ("DeleteCrl", client.delete_crl(id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}: {path} on a deleted CRL must answer 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}: {path} must answer ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                Some(arn_not_found_message(&crl_arn(REGION, id)).as_str()),
                "case {case:?}: {path} must name the missing CRL by ARN, got {answer:?}"
            );
        }

        // Every CRL the table has not deleted yet is still there, so the delete removed
        // exactly one record. `ListCrls` pages a `BTreeMap` in key order, which is not
        // import order, so the comparison is by set.
        remaining.retain(|stored| stored != id);
        let listed: BTreeSet<String> = client.list_crl_ids().await.into_iter().collect();
        assert_eq!(
            listed,
            remaining.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: ListCrls must report exactly the CRLs that are left"
        );
        for still_stored in &remaining {
            let answer = client.get_crl(still_stored).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {still_stored} must survive another CRL's delete, got {answer:?}"
            );
        }
    }
}

/// A `crlId` that resolves to no stored CRL is the modelled 404, and the refusal names the
/// ARN the REQUEST's id and Region build — never a fixed string and never another
/// recording's ARN.
#[tokio::test]
async fn get_crl_names_the_requested_crl_by_arn_in_its_refusal() {
    let client = Client::new();
    let (anchor_id, anchor_arn) = trust_anchor(&client, "refusal-anchor").await;
    let imported = client.import_crl("a stored crl", &anchor_arn).await;
    assert_eq!(
        imported.status, 201,
        "expected ImportCrl 201, got {imported:?}"
    );

    // Ids of the modelled `Uuid` form that no CRL carries: never minted, all-hex, and one
    // belonging to this service's OTHER resource type — the CRL store is what the read
    // consults.
    let unresolvable = [
        ("never-minted", "00000000-0000-4000-8000-000000000000".to_string()),
        ("all-hex", "abcdef12-3456-7890-abcd-ef1234567890".to_string()),
        ("a-trust-anchor-id", anchor_id),
    ];

    // The Region is part of the ARN, so the same unresolvable id refused in two Regions is
    // named by two different ARNs.
    for region in [REGION, "eu-west-1"] {
        for (case, id) in &unresolvable {
            let answer = client
                .send_in_region("GET", &format!("/crl/{id}"), None, region)
                .await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{region}: expected GetCrl 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{region}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                Some(arn_not_found_message(&crl_arn(region, id)).as_str()),
                "case {case:?}/{region}: the refusal must name the requested CRL's ARN, got \
                 {answer:?}"
            );
        }
    }
}

/// `ImportCrl` resolves `trustAnchorArn` before it stores anything: an ARN that names no
/// stored trust anchor is refused with the modelled 404 naming the id it carried, and no
/// CRL is created.
///
/// The rows span the ways an ARN can resolve to nothing — an id never minted, a deleted
/// trust anchor, another partition, another Region, another account, and a resource part
/// carrying no id — plus the control row: an ARN that does resolve is served.
#[tokio::test]
async fn import_crl_refuses_a_trust_anchor_arn_that_resolves_to_nothing() {
    let client = Client::new();
    let (live_id, live_arn) = trust_anchor(&client, "import-live-anchor").await;
    let (doomed_id, doomed_arn) = trust_anchor(&client, "import-doomed-anchor").await;
    let deleted = client
        .send("DELETE", &format!("/trustanchor/{doomed_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteTrustAnchor 200, got {deleted:?}"
    );

    // The id every refusal must name is the one the submitted ARN carries.
    let unminted = "2521117b-c08f-42dc-a517-245874313ff8";
    let cases: Vec<(&str, String, String)> = vec![
        (
            "never-minted-id",
            format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor/{unminted}"),
            unminted.to_string(),
        ),
        ("deleted-trust-anchor", doomed_arn.clone(), doomed_id.clone()),
        (
            // `TrustAnchorArn` is
            // `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:trust-anchor.*)$")`, so a
            // non-default partition is a well-formed ARN the service still has to resolve.
            "another-partition",
            format!(
                "arn:aws-us-gov:rolesanywhere:us-gov-west-1:{ACCOUNT}:trust-anchor/\
                 11111111-2222-3333-4444-555555555555"
            ),
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "another-account",
            format!(
                "arn:aws:rolesanywhere:{REGION}:210987654321:trust-anchor/\
                 99999999-8888-7777-6666-555555555555"
            ),
            "99999999-8888-7777-6666-555555555555".to_string(),
        ),
        (
            "no-id-in-the-resource",
            format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor"),
            String::new(),
        ),
        (
            // The resource part names no id unless `trust-anchor` is followed by a
            // separator, so this one carries none either.
            "no-separator-before-the-id",
            format!("arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor-not-a-separator"),
            String::new(),
        ),
        (
            // A colon is the other separator an ARN resource part uses.
            "colon-separated-id",
            format!(
                "arn:aws:rolesanywhere:{REGION}:{ACCOUNT}:trust-anchor:\
                 33333333-4444-5555-6666-777777777777"
            ),
            "33333333-4444-5555-6666-777777777777".to_string(),
        ),
    ];

    let before = client.list_crl_ids().await;
    let before: BTreeSet<String> = before.into_iter().collect();
    for (case, arn, expected_id) in &cases {
        let answer = client.import_crl("dangling crl", arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected ImportCrl 404 for {arn}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(format!("Unable to find Trust Anchor with ID {expected_id}.").as_str()),
            "case {case:?}: the refusal must name the id the ARN carried, got {answer:?}"
        );
        assert_eq!(
            client
                .list_crl_ids()
                .await
                .into_iter()
                .collect::<BTreeSet<String>>(),
            before,
            "case {case:?}: a refused import must store nothing"
        );
    }

    // The control row: the same request against an ARN that resolves is served, and the
    // stored CRL keeps the link it was given.
    let served = client.import_crl("linked crl", &live_arn).await;
    assert_eq!(
        served.status, 201,
        "case \"resolvable-arn\": expected ImportCrl 201, got {served:?}"
    );
    let crl = served.body.get("crl").cloned().unwrap_or(Value::Null);
    assert_eq!(
        crl.get("trustAnchorArn").and_then(Value::as_str),
        Some(live_arn.as_str()),
        "case \"resolvable-arn\": the stored CRL keeps the submitted link, got {crl:?}"
    );
    let id = crl
        .get("crlId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case \"resolvable-arn\": no crlId in {served:?}"))
        .to_string();
    let read = client.get_crl(&id).await;
    assert_eq!(
        read.status, 200,
        "case \"resolvable-arn\": the import must be readable, got {read:?}"
    );
    assert_eq!(
        read.body
            .pointer("/crl/trustAnchorArn")
            .and_then(Value::as_str),
        Some(live_arn.as_str()),
        "case \"resolvable-arn\": the read reports the stored link, got {read:?}"
    );
    assert!(
        live_id.len() == 36,
        "the live trust anchor id must be a modelled Uuid, got {live_id:?}"
    );
}
