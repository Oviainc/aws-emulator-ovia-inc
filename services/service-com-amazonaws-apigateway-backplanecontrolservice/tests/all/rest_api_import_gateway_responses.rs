// SPDX-License-Identifier: CC0-1.0

//! The gateway responses an imported API DEFINITION customizes, read back through
//! `GetGatewayResponse` and `GetGatewayResponses`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` helper are this crate's existing harness, reused from
//! `rest_api_lifecycle` and `gateway_response`.
//!
//! The RULE under test: a definition carrying API Gateway's
//! `x-amazon-apigateway-gateway-responses` extension creates the gateway responses it
//! declares — their status code, their response parameters and their response templates —
//! with the AWS pseudo parameters of the template text RESOLVED against the account and
//! Region the API is imported into, and resolves them AGAIN on a later import, because
//! the record is written from the document as read in that request's scope. It holds for
//! both operations that import a definition (`ImportRestApi` creating an API and
//! `PutRestApi` updating one) and for both `PutMode` values, where `overwrite` replaces
//! the API's own customizations and `merge` adds to them.
//!
//! The rows span what the rule generalises over: which response types the document
//! declares, which of the three members each entry carries, how many templates and
//! parameters an entry declares, which pseudo parameter the text uses, and a response
//! type outside the modelled enum.

use serde_json::Value;

use crate::gateway_response::get_gateway_response;
use crate::rest_api_lifecycle::{create_api, string_member, Answer, Gateway};

/// The Region every request of this harness is scoped to, which is the Region an import
/// resolves `${AWS::Region}` to. Source: the credential scope
/// `rest_api_lifecycle::Gateway` signs with.
const REGION: &str = "us-east-1";

/// The partition this emulator's ARNs are written in, which `${AWS::Partition}` resolves
/// to.
const PARTITION: &str = "aws";

/// `PutRestApi` with `body` sent verbatim as the definition payload, in `mode`.
async fn put_definition(gateway: &Gateway, api: &str, mode: &str, body: &str) -> Answer {
    gateway
        .call_raw("PUT", &format!("/restapis/{api}?mode={mode}"), body)
        .await
}

/// `ImportRestApi` with `body` sent verbatim as the definition payload.
async fn import_definition(gateway: &Gateway, body: &str) -> Answer {
    gateway.call_raw("POST", "/restapis?mode=import", body).await
}

/// An OpenAPI document whose only content is the gateway-responses extension `responses`
/// spells, so what is asserted is what that extension became.
fn definition(title: &str, responses: &str) -> String {
    format!(
        "{{\"openapi\": \"3.0.1\", \"info\": {{\"title\": \"{title}\"}}, \
          \"paths\": {{}}, \
          \"x-amazon-apigateway-gateway-responses\": {{{responses}}}}}"
    )
}

/// The template text a document declares, with the pseudo parameters written out, and
/// the text the import must store once they are resolved.
fn template(account: &str) -> (String, String) {
    (
        "{\\\"message\\\":$context.error.messageString,\
          \\\"region\\\":\\\"${AWS::Region}\\\",\
          \\\"account\\\":\\\"${AWS::AccountId}\\\",\
          \\\"partition\\\":\\\"${AWS::Partition}\\\"}"
            .to_string(),
        format!(
            "{{\"message\":$context.error.messageString,\
              \"region\":\"{REGION}\",\
              \"account\":\"{account}\",\
              \"partition\":\"{PARTITION}\"}}"
        ),
    )
}

/// The gateway response `response_type` reports for `api`, asserting the modelled 200.
async fn read(gateway: &Gateway, api: &str, response_type: &str, case: &str) -> Value {
    let answer = get_gateway_response(gateway, api, response_type).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from GetGatewayResponse({response_type}), got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

#[tokio::test]
async fn an_imported_definitions_gateway_responses_are_created_with_their_templates() {
    let gateway = Gateway::new();
    let account = engine::emulator_account_id();
    let (declared, resolved) = template(account);

    // (case, the response type, the extension entry, the members the record must carry:
    // the status code, the template content types and the parameter keys)
    type Row<'a> = (&'a str, &'a str, String, Option<&'a str>, Vec<&'a str>, Vec<&'a str>);
    let rows: Vec<Row> = vec![
        (
            "a type with a status code, a template and a parameter",
            "DEFAULT_4XX",
            format!(
                "\"statusCode\": \"400\", \
                 \"responseParameters\": {{\"gatewayresponse.header.x-source\": \"'import'\"}}, \
                 \"responseTemplates\": {{\"application/json\": \"{declared}\"}}"
            ),
            Some("400"),
            vec!["application/json"],
            vec!["gatewayresponse.header.x-source"],
        ),
        (
            "a type carrying two templates",
            "DEFAULT_5XX",
            format!(
                "\"statusCode\": \"500\", \
                 \"responseTemplates\": {{\"application/json\": \"{declared}\", \
                                          \"text/plain\": \"{declared}\"}}"
            ),
            Some("500"),
            vec!["application/json", "text/plain"],
            Vec::new(),
        ),
        (
            // No `statusCode` of its own: the record keeps the one the type's generated
            // response carries, exactly as a `PutGatewayResponse` that set none does.
            "a type declaring only a template",
            "UNAUTHORIZED",
            format!("\"responseTemplates\": {{\"application/json\": \"{declared}\"}}"),
            Some("401"),
            vec!["application/json"],
            Vec::new(),
        ),
        (
            // A type the catalog gives no status code at all, and an entry that declares
            // none either.
            "a type with neither a declared nor a generated status code",
            "DEFAULT_4XX",
            format!("\"responseTemplates\": {{\"application/json\": \"{declared}\"}}"),
            None,
            vec!["application/json"],
            Vec::new(),
        ),
    ];

    for (case, response_type, entry, status_code, content_types, parameter_keys) in rows {
        let document = definition(case, &format!("\"{response_type}\": {{{entry}}}"));
        // Both operations that import a definition must leave the same record behind, so
        // each row is driven through each of them.
        let created = import_definition(&gateway, &document).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from ImportRestApi, got {} with {:?}",
            created.status, created.body
        );
        let imported = string_member(&created.body, "id", case);
        let (updated, _root) = create_api(&gateway, &format!("put-{case}")).await;
        let put = put_definition(&gateway, &updated, "overwrite", &document).await;
        assert_eq!(
            put.status, 200,
            "case {case}: expected 200 from PutRestApi, got {} with {:?}",
            put.status, put.body
        );

        for (whose, api) in [("ImportRestApi", &imported), ("PutRestApi", &updated)] {
            let case = format!("{case} [{whose}]");
            let record = read(&gateway, api, response_type, &case).await;
            assert_eq!(
                record["defaultResponse"].as_bool(),
                Some(false),
                "case {case}: an imported record is a customization, got {record:?}"
            );
            assert_eq!(
                record["statusCode"].as_str(),
                status_code,
                "case {case}: expected the status code {status_code:?}, got {:?}",
                record["statusCode"]
            );
            let templates = record["responseTemplates"]
                .as_object()
                .unwrap_or_else(|| panic!("case {case}: expected responseTemplates, got {record:?}"));
            assert_eq!(
                templates.len(),
                content_types.len(),
                "case {case}: expected {} template(s), got {templates:?}",
                content_types.len()
            );
            for content_type in &content_types {
                assert_eq!(
                    templates.get(*content_type).and_then(Value::as_str),
                    Some(resolved.as_str()),
                    "case {case}: expected the {content_type} template with its pseudo \
                     parameters resolved, got {templates:?}"
                );
            }
            match parameter_keys.is_empty() {
                true => assert_eq!(
                    record["responseParameters"],
                    ::serde_json::json!({}),
                    "case {case}: expected no response parameters, got {:?}",
                    record["responseParameters"]
                ),
                false => {
                    for key in &parameter_keys {
                        assert_eq!(
                            record["responseParameters"][*key].as_str(),
                            Some("'import'"),
                            "case {case}: expected the response parameter {key}, got {:?}",
                            record["responseParameters"]
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn an_overwrite_import_re_resolves_the_templates_and_drops_what_it_omits() {
    let gateway = Gateway::new();
    let account = engine::emulator_account_id();
    let (declared, resolved) = template(account);

    let (api, _root) = create_api(&gateway, "gateway-responses-overwrite").await;
    let first = definition(
        "two types",
        &format!(
            "\"DEFAULT_4XX\": {{\"statusCode\": \"400\", \
                \"responseTemplates\": {{\"application/json\": \"{declared}\"}}}}, \
             \"UNAUTHORIZED\": {{\"statusCode\": \"401\", \
                \"responseTemplates\": {{\"application/json\": \"{declared}\"}}}}"
        ),
    );
    let answer = put_definition(&gateway, &api, "overwrite", &first).await;
    assert_eq!(
        answer.status, 200,
        "case the first import: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    for response_type in ["DEFAULT_4XX", "UNAUTHORIZED"] {
        let record = read(&gateway, &api, response_type, "the first import").await;
        assert_eq!(
            record["responseTemplates"]["application/json"].as_str(),
            Some(resolved.as_str()),
            "case the first import ({response_type}): expected the resolved template, \
             got {record:?}"
        );
    }

    // The SAME document imported again over the same API: the templates are resolved
    // once more, from the document as read in this request's scope, so the record is the
    // same one rather than carrying a parameter through.
    let answer = put_definition(&gateway, &api, "overwrite", &first).await;
    assert_eq!(
        answer.status, 200,
        "case the re-import: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let record = read(&gateway, &api, "DEFAULT_4XX", "the re-import").await;
    assert_eq!(
        record["responseTemplates"]["application/json"].as_str(),
        Some(resolved.as_str()),
        "case the re-import: expected the template resolved again, got {record:?}"
    );

    // An `overwrite` import REPLACES the API's own definition, so a type the new
    // document says nothing about goes back to the response API Gateway generates.
    let second = definition(
        "one type",
        &format!(
            "\"DEFAULT_4XX\": {{\"statusCode\": \"404\", \
                \"responseTemplates\": {{\"text/plain\": \"{declared}\"}}}}"
        ),
    );
    let answer = put_definition(&gateway, &api, "overwrite", &second).await;
    assert_eq!(
        answer.status, 200,
        "case the narrowing import: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let kept = read(&gateway, &api, "DEFAULT_4XX", "the narrowing import").await;
    assert_eq!(
        kept["statusCode"].as_str(),
        Some("404"),
        "case the narrowing import: expected the newly declared status code, got {kept:?}"
    );
    assert_eq!(
        kept["responseTemplates"]["text/plain"].as_str(),
        Some(resolved.as_str()),
        "case the narrowing import: expected the newly declared template, got {kept:?}"
    );
    assert!(
        kept["responseTemplates"]["application/json"].is_null(),
        "case the narrowing import: expected the replaced template to be gone, got {kept:?}"
    );
    let dropped = read(&gateway, &api, "UNAUTHORIZED", "the narrowing import").await;
    assert_eq!(
        dropped["defaultResponse"].as_bool(),
        Some(true),
        "case the narrowing import: expected the omitted type to go back to the \
         generated response, got {dropped:?}"
    );

    // A `merge` import adds to what the API already carries instead of replacing it.
    let merged = definition(
        "merged",
        &format!(
            "\"UNAUTHORIZED\": {{\"statusCode\": \"401\", \
                \"responseTemplates\": {{\"application/json\": \"{declared}\"}}}}"
        ),
    );
    let answer = put_definition(&gateway, &api, "merge", &merged).await;
    assert_eq!(
        answer.status, 200,
        "case the merge import: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    for (response_type, content_type) in [
        ("DEFAULT_4XX", "text/plain"),
        ("UNAUTHORIZED", "application/json"),
    ] {
        let record = read(&gateway, &api, response_type, "the merge import").await;
        assert_eq!(
            record["responseTemplates"][content_type].as_str(),
            Some(resolved.as_str()),
            "case the merge import ({response_type}): expected the customization to \
             stand, got {record:?}"
        );
    }
}

#[tokio::test]
async fn a_gateway_response_type_outside_the_modelled_enum_is_reported_and_not_stored() {
    let gateway = Gateway::new();
    let (api, _root) = create_api(&gateway, "gateway-responses-unknown-type").await;

    // Rows differ along the way a key can miss the modelled `GatewayResponseType` enum: a
    // value shaped like one of its values, a declared value in the wrong case, and an
    // unknown word. Each customizes nothing, so it is reported in `warnings` rather than
    // stored, and the entry beside it is still imported.
    for unknown in ["DEFAULT_3XX", "unauthorized", "NOT_A_RESPONSE_TYPE"] {
        let document = definition(
            unknown,
            &format!(
                "\"{unknown}\": {{\"statusCode\": \"302\"}}, \
                 \"DEFAULT_5XX\": {{\"statusCode\": \"502\"}}"
            ),
        );
        let answer = put_definition(&gateway, &api, "overwrite", &document).await;
        assert_eq!(
            answer.status, 200,
            "case {unknown}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let warnings = answer.body["warnings"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_owned))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();
        assert!(
            warnings.iter().any(|warning| warning.contains(unknown)),
            "case {unknown}: expected the import to report the unknown type, got {warnings:?}"
        );
        // The entry beside it was still imported.
        let stored = read(&gateway, &api, "DEFAULT_5XX", unknown).await;
        assert_eq!(
            stored["statusCode"].as_str(),
            Some("502"),
            "case {unknown}: expected the declared type beside it to be imported, got {stored:?}"
        );
    }
}
