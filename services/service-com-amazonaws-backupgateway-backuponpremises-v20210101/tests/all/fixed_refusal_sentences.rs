// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two refusal SENTENCES this service states as fixed text: the
//! one `GetHypervisorPropertyMappings` states for a hypervisor carrying no property
//! mappings, and the one `StartVirtualMachinesMetadataSync` states for a `HypervisorArn`
//! whose property mappings are not configured.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle` and the workflow helpers of
//! `hypervisor_property_mappings` and `metadata_sync_over_the_wire`, so what is asserted is
//! the wire behaviour.
//!
//! The rule under test: where the sentence AWS states for a refusal class is entirely FIXED
//! text, this service states that one sentence verbatim for every request reaching the
//! class — no part of it is taken from the request, and it does not vary with which
//! resource the request names (or whether the ARN names one at all), with how far along the
//! workflow the record was taken, or with what earlier calls did.
//!
//! The two sentences are written out HERE as literals rather than read off the constants
//! the handlers quote (`backup_gateway::state::MAPPINGS_NOT_FOUND_MESSAGE`,
//! `MAPPINGS_NOT_CONFIGURED_MESSAGE`), because a sentence is only pinned by a test that
//! does not get it from the code under test: re-wording either constant has to fail here.
//! Neither sentence is published by the model —
//! `smithy_query(query="shape",shape="ResourceNotFoundException")` and
//! `smithy_query(query="shape",shape="AccessDeniedException")` document only the required
//! `ErrorCode` member ("A description of which resource wasn't found", "A description of why
//! you have insufficient permissions") and carry no `Message` wording — so each is the
//! sentence AWS is recorded stating for its condition, pinned by the recorded units of
//! `issues/things-that-now-work/20261029T195704Z-backup-gateway-two-backup-gateway-refusal-sentences-are-the-crate.md`.
//!
//! The tables below span each class rather than one request: records differing in host
//! form, in how far along the documented workflow they were taken, in whether a refused
//! `PutHypervisorPropertyMappings` preceded the read and in whether mappings were ever
//! held; and ARNs differing in resource type, partition, id and length, including each
//! published bound of `ServerArn`.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    StoredPropertyMappings, GATEWAY_OUT_OF_DATE_MESSAGE, HYPERVISOR_NOT_FOUND,
    HYPERVISOR_NOT_FOUND_MESSAGE, MAPPINGS_NOT_CONFIGURED, MAPPINGS_NOT_CONFIGURED_STATUS,
    MAPPINGS_NOT_FOUND,
};

use crate::gateway_lifecycle::{build_engine, build_router, call, create, Reply};
use crate::hypervisor_property_mappings::{
    arn_of_length, get_mappings, hypervisor_arn_in, import, mapping, set_mappings,
};
use crate::metadata_sync_over_the_wire::{associate_gateway, role_arn, start_sync};
use crate::metadata_sync_requires_property_mappings::refusal;

/// The sentence AWS states when the hypervisor property mappings asked for are not found —
/// entirely fixed text, its number disagreement ("mappings is") included, with no part
/// taken from the request.
const MAPPINGS_NOT_FOUND_SENTENCE: &str = "The hypervisor property mappings is not found.";

/// The sentence AWS states when a metadata sync is asked for a hypervisor whose property
/// mappings are not configured — entirely fixed text, with no trailing period and no part
/// taken from the request.
const MAPPINGS_NOT_CONFIGURED_SENTENCE: &str =
    "HypervisorPropertyMappings is not configured for the Hypervisor";

/// A sentence that is fixed text carries NOTHING of the request: none of `request_values`
/// (the ARN asked for, its id segment, the record's name and host) appears in it.
fn assert_nothing_of_the_request_reached(case: &str, reply: &Reply, request_values: &[&str]) {
    let message = reply.str_at("/Message").unwrap_or_default();
    for value in request_values {
        assert!(
            !value.is_empty() && !message.contains(value),
            "case {case:?}: a fixed sentence must carry no part of the request, but \
             {message:?} contains {value:?}"
        );
    }
}

/// The id segment of `arn` — the part a message that echoed the resource would most likely
/// carry.
fn arn_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// `GetHypervisorPropertyMappings` states ONE fixed sentence for every hypervisor it holds
/// no property mappings for, whatever the record carries beside them and whatever earlier
/// calls did to it — and that sentence names the hypervisor property mappings, so it is not
/// the sentence the service states for a missing hypervisor.
#[tokio::test]
async fn the_missing_property_mappings_refusal_states_one_fixed_sentence() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, Name, Host, whether a gateway is created for it, whether a refused put
    // precedes the read, whether mappings were held and then cleared). The rows differ in
    // what the record carries beside its mappings and in what happened to it before the
    // read — the refusal keys on none of it.
    let rows: [(&str, &str, &str, bool, bool, bool); 5] = [
        (
            "freshly imported, an IPv4 host",
            "hv-sentence-0",
            "198.51.100.60",
            false,
            false,
            false,
        ),
        (
            "freshly imported, an FQDN host",
            "hv-sentence-1",
            "hv-sentence-1.example.org",
            false,
            false,
            false,
        ),
        (
            "read after a put that was refused as an out-of-date gateway",
            "hv-sentence-2",
            "198.51.100.61",
            false,
            true,
            false,
        ),
        (
            "a gateway of its own exists but serves no hypervisor",
            "hv-sentence-3",
            "198.51.100.62",
            true,
            false,
            false,
        ),
        (
            "mappings were held and then cleared",
            "hv-sentence-4",
            "198.51.100.63",
            false,
            false,
            true,
        ),
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();

    for (case, name, host, own_gateway, refused_put, cleared) in rows {
        let arn = import(&router, name, host).await;
        if own_gateway {
            create(&router, &format!("gw-{name}")).await;
        }
        if refused_put {
            // A put for a hypervisor no gateway serves is refused, so this leg reaches the
            // read with a rejected write behind it — exactly as the recorded unit does.
            let put = call(
                &router,
                "PutHypervisorPropertyMappings",
                &json!({
                    "HypervisorArn": arn,
                    "IamRoleArn": role_arn("aws", "sentence-refused"),
                    "VmwareToAwsTagMappings": [],
                }),
            )
            .await;
            assert_eq!(
                (put.status, put.str_at("/Message")),
                (400, Some(GATEWAY_OUT_OF_DATE_MESSAGE)),
                "case {case:?}: the put must be refused as an out-of-date gateway, got {} {:?}",
                put.status,
                put.body
            );
        }
        if cleared {
            set_mappings(
                &engine,
                &arn,
                Some(StoredPropertyMappings {
                    iam_role_arn: role_arn("aws", "sentence-cleared"),
                    vmware_to_aws_tag_mappings: vec![mapping("tier", "gold", "Tier", "Gold")],
                }),
            );
            set_mappings(&engine, &arn, None);
        }

        let reply = get_mappings(&router, &arn).await;
        assert_eq!(
            refusal(&reply),
            (
                400,
                Some("ResourceNotFoundException"),
                Some(MAPPINGS_NOT_FOUND),
                Some(MAPPINGS_NOT_FOUND_SENTENCE),
            ),
            "case {case:?}: a hypervisor with no mappings must be refused with the fixed \
             sentence {MAPPINGS_NOT_FOUND_SENTENCE:?}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_nothing_of_the_request_reached(case, &reply, &[&arn, arn_id(&arn), name, host]);
        sentences.insert(reply.str_at("/Message").unwrap_or_default().to_string());

        // Repeating the read states the very same sentence: it is fixed text, not something
        // the first answer settled.
        let again = get_mappings(&router, &arn).await;
        assert_eq!(
            again.str_at("/Message"),
            Some(MAPPINGS_NOT_FOUND_SENTENCE),
            "case {case:?}: a repeated read must state the same sentence, got {:?}",
            again.body
        );
    }

    assert_eq!(
        sentences.len(),
        1,
        "every row of the class must state one sentence, got {sentences:?}"
    );

    // The two conditions this operation refuses are named APART: an ARN naming no record at
    // all takes the missing-hypervisor sentence, which is not the one above, so the
    // mappings sentence names the mappings rather than an unqualified resource.
    let ghost = hypervisor_arn_in("aws", "hv-ffffffffffffff30");
    let missing = get_mappings(&router, &ghost).await;
    assert_eq!(
        refusal(&missing),
        (
            400,
            Some("ResourceNotFoundException"),
            Some(HYPERVISOR_NOT_FOUND),
            Some(HYPERVISOR_NOT_FOUND_MESSAGE),
        ),
        "an ARN naming no record must be refused as a missing hypervisor, got {} {:?}",
        missing.status,
        missing.body
    );
    assert_ne!(
        MAPPINGS_NOT_FOUND_SENTENCE, HYPERVISOR_NOT_FOUND_MESSAGE,
        "the missing-mappings sentence must not be the missing-hypervisor one"
    );
    assert!(
        MAPPINGS_NOT_FOUND_SENTENCE.contains("hypervisor property mappings"),
        "the missing-mappings sentence must name the hypervisor property mappings, got \
         {MAPPINGS_NOT_FOUND_SENTENCE:?}"
    );
}

/// `StartVirtualMachinesMetadataSync` states ONE fixed sentence for every `HypervisorArn`
/// whose property mappings are not configured — a record the store holds and a well-formed
/// ARN naming nothing alike, in every partition the modelled `ServerArn` accepts and at
/// each of its published length bounds — and that sentence says the mappings are not
/// configured rather than restating the shape's generic permissions wording.
#[tokio::test]
async fn the_mappings_not_configured_refusal_states_one_fixed_sentence() {
    let engine = build_engine();
    let router = build_router(&engine);

    // Records that really exist, so a refusal below is about the mappings the sync needs
    // and not about an empty store.
    let live_gateway = create(&router, "gw-sentence-live").await;
    let imported_only = import(&router, "hv-sync-sentence-bare", "198.51.100.70").await;
    let served_only = import(&router, "hv-sync-sentence-served", "198.51.100.71").await;
    associate_gateway(&router, "gw-hv-sync-sentence-served", &served_only).await;

    let rows: Vec<(&str, String)> = vec![
        (
            "a hypervisor that was only imported, so no mappings were put",
            imported_only,
        ),
        (
            "a hypervisor a gateway serves, with no mappings put",
            served_only,
        ),
        ("a gateway ARN that really names a gateway", live_gateway),
        (
            "a hypervisor ARN that was never imported",
            hypervisor_arn_in("aws", "hv-ffffffffffffff31"),
        ),
        (
            "a never-imported ARN in the aws-cn partition",
            hypervisor_arn_in("aws-cn", "hv-ffffffffffffff32"),
        ),
        (
            "a never-imported ARN in the aws-us-gov partition",
            hypervisor_arn_in("aws-us-gov", "hv-ffffffffffffff33"),
        ),
        (
            "the 50-character minimum of the modelled ServerArn",
            arn_of_length(50),
        ),
        (
            "the 500-character maximum of the modelled ServerArn",
            arn_of_length(500),
        ),
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();

    for (case, arn) in &rows {
        let reply = start_sync(&router, arn).await;
        assert_eq!(
            refusal(&reply),
            (
                MAPPINGS_NOT_CONFIGURED_STATUS,
                Some("AccessDeniedException"),
                Some(MAPPINGS_NOT_CONFIGURED),
                Some(MAPPINGS_NOT_CONFIGURED_SENTENCE),
            ),
            "case {case:?}: a sync for an ARN with no configured mappings must be refused \
             with the fixed sentence {MAPPINGS_NOT_CONFIGURED_SENTENCE:?}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_nothing_of_the_request_reached(case, &reply, &[arn, arn_id(arn)]);
        sentences.insert(reply.str_at("/Message").unwrap_or_default().to_string());
    }

    assert_eq!(
        sentences.len(),
        1,
        "every row of the class must state one sentence, got {sentences:?}"
    );

    // The sentence tells the caller WHY the sync was denied — the hypervisor's property
    // mappings — and not that it holds insufficient permissions, which is the remediation
    // the other reading would send it to.
    let sentence = sentences
        .iter()
        .next()
        .expect("one sentence was collected above");
    assert!(
        sentence.contains("HypervisorPropertyMappings") && sentence.contains("not configured"),
        "the refusal must name the property mappings as not configured, got {sentence:?}"
    );
    assert!(
        !sentence.ends_with('.'),
        "the recorded sentence carries no trailing period, got {sentence:?}"
    );
}
