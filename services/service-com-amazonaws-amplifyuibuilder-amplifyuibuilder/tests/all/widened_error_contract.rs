// SPDX-License-Identifier: CC0-1.0

//! The STATED error contract of an operation whose error channel is widened.
//!
//! The RULE: an operation's `## Errors` contract block states every error its handler
//! can answer, with that error shape's modelled HTTP status — including one the model
//! does not declare for the operation and that the handler widens into the channel
//! through `extra_errors`, which is marked as widened so a reader can tell the stated
//! contract from the modelled one. A widening the block omits makes the file's own
//! contract contradict the status the handler answers.
//!
//! The rows span what the rule generalises over: EVERY operation module of this crate
//! (each is checked, whichever errors it declares, widens or documents), every error
//! shape this service models, and — for the three update operations, whose widened
//! refusal a request can actually provoke — the FORM of the unresolvable id the refusal
//! is provoked with. The last test closes the loop the issue is about: the status the
//! block states is the status the emulator answers on the wire, so a block that states
//! nothing cannot agree with it.
//!
//! The modelled statuses the catalogue below carries come from
//! `smithy_query(query="shape",shape="InternalServerException,InvalidParameterException,ResourceConflictException,ResourceNotFoundException,ServiceQuotaExceededException,ThrottlingException,UnauthorizedException")`.
//!
//! Each suite in this crate carries its own request harness, so this one does too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Every error shape this service models, with the HTTP status its `@httpError` trait
/// gives it (see the `smithy_query` call in this module's docs). A documented status
/// is checked against this catalogue, so a bullet cannot state a status the model
/// does not give the shape.
const MODELLED_ERROR_STATUS: [(&str, u16); 7] = [
    ("InternalServerException", 500),
    ("InvalidParameterException", 400),
    ("ResourceConflictException", 409),
    ("ResourceNotFoundException", 404),
    ("ServiceQuotaExceededException", 402),
    ("ThrottlingException", 429),
    ("UnauthorizedException", 401),
];

/// The phrase a bullet carries to say the shape is not one the model declares for this
/// operation but one the handler widens in.
const WIDENED_MARK: &str = "widened in through `extra_errors`";

/// One bullet of an operation module's `## Errors` block, with its continuation lines
/// joined onto it.
#[derive(Debug, Clone)]
struct ErrorBullet {
    /// The error shape the bullet is about: its first backticked word.
    name: String,
    /// The status the bullet states, from its `(HTTP nnn)`, when it states one.
    status: Option<u16>,
    /// Whether the bullet says the shape is widened into the channel.
    widened: bool,
}

/// One operation module: what its `## Errors` block states, and what its
/// `#[operation_error]` attribute actually widens in.
#[derive(Debug)]
struct OperationModule {
    /// The file name, e.g. `update_form.rs`.
    file: String,
    /// The bullets of its `## Errors` block, empty when it has no such block.
    documented: Vec<ErrorBullet>,
    /// The shapes its `extra_errors = [...]` names, empty when it widens nothing.
    widened: BTreeSet<String>,
}

impl OperationModule {
    fn bullet(&self, name: &str) -> Option<&ErrorBullet> {
        self.documented.iter().find(|bullet| bullet.name == name)
    }
}

fn ops_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ops")
}

/// Every `src/ops/<operation>.rs` of this crate, parsed. `mod.rs` is the module list,
/// not an operation, and `types/` is a directory, so neither is a row.
fn operation_modules() -> Vec<OperationModule> {
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(ops_dir()).expect("this crate's src/ops directory") {
        let path = entry.expect("an entry of src/ops").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let file = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if file == "mod.rs" {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {file}: {error}"));
        modules.push(OperationModule {
            file,
            documented: documented_errors(&source),
            widened: widened_errors(&source),
        });
    }
    modules.sort_by(|left, right| left.file.cmp(&right.file));
    modules
}

/// The bullets of the module doc's `## Errors` section. A bullet starts at `* ` and
/// owns the indented lines that follow it, which is how a clipped doc entry wraps.
fn documented_errors(source: &str) -> Vec<ErrorBullet> {
    let mut bullets: Vec<String> = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        let Some(doc) = line.strip_prefix("//!") else {
            continue;
        };
        let doc = doc.trim();
        if doc.starts_with("##") || doc.starts_with("# ") {
            inside = doc == "## Errors";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(bullet) = doc.strip_prefix("* ") {
            bullets.push(bullet.to_string());
        } else if !doc.is_empty() {
            if let Some(last) = bullets.last_mut() {
                last.push(' ');
                last.push_str(doc);
            }
        }
    }

    bullets
        .iter()
        .map(|text| ErrorBullet {
            name: backticked(text).unwrap_or_default(),
            status: stated_status(text),
            widened: text.contains(WIDENED_MARK),
        })
        .collect()
}

/// The first backticked word of a bullet: the error shape it is about.
fn backticked(text: &str) -> Option<String> {
    let rest = text.strip_prefix('`')?;
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

/// The status a bullet states, from `(HTTP nnn)`.
fn stated_status(text: &str) -> Option<u16> {
    let rest = text.split_once("(HTTP ")?.1;
    let digits = rest.split_once(')')?.0.trim();
    digits.parse().ok()
}

/// The shapes an `#[operation_error(..., extra_errors = ["A", "B"])]` attribute widens
/// into the operation's error channel.
fn widened_errors(source: &str) -> BTreeSet<String> {
    let mut widened = BTreeSet::new();
    for attribute in source.split("extra_errors").skip(1) {
        // Only the attribute's own `extra_errors = [...]` is a widening; the same word
        // inside a doc bullet or a comment is prose and is skipped here.
        let Some(rest) = attribute.trim_start().strip_prefix('=') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('[') else {
            continue;
        };
        let Some((list, _)) = rest.split_once(']') else {
            continue;
        };
        for quoted in list.split(',') {
            let name = quoted.trim().trim_matches('"').trim();
            if !name.is_empty() {
                widened.insert(name.to_string());
            }
        }
    }
    widened
}

fn modelled_status(name: &str) -> Option<u16> {
    MODELLED_ERROR_STATUS
        .iter()
        .find(|(shape, _)| *shape == name)
        .map(|(_, status)| *status)
}

/// Every status an operation module's `## Errors` block states is the status the model
/// gives that error shape — on every module of the crate, widening or not.
#[test]
fn every_documented_error_states_its_modelled_status() {
    let modules = operation_modules();
    assert!(
        modules.len() >= 20,
        "the src/ops scan must find this service's operation modules, found {}: {:?}",
        modules.len(),
        modules.iter().map(|module| &module.file).collect::<Vec<_>>()
    );

    let mut with_errors = 0usize;
    for module in &modules {
        if !module.documented.is_empty() {
            with_errors += 1;
        }
        for bullet in &module.documented {
            let case = format!("{} documents {}", module.file, bullet.name);
            let expected = modelled_status(&bullet.name);
            assert!(
                expected.is_some(),
                "case {case:?}: expected an error shape this service models, got {:?}",
                bullet.name
            );
            assert_eq!(
                bullet.status, expected,
                "case {case:?}: expected the modelled status {expected:?}, got {:?}",
                bullet.status
            );
        }
    }
    assert!(
        with_errors >= 20,
        "the parse must find the ## Errors block of this service's operations, found {with_errors}"
    );
}

/// An operation that widens an error in states it in its own contract block, marked as
/// widened — and nothing else in that block is marked widened. Every module of the
/// crate is a row, so a handler that starts widening without saying so fails here.
#[test]
fn every_widened_error_is_stated_in_the_contract_block() {
    let modules = operation_modules();
    let widening: Vec<&OperationModule> = modules
        .iter()
        .filter(|module| !module.widened.is_empty())
        .collect();
    assert!(
        !widening.is_empty(),
        "this service's widening handlers must be found by the src/ops scan, found none"
    );

    for module in &modules {
        for name in &module.widened {
            let case = format!("{} widens {name}", module.file);
            let bullet = module.bullet(name);
            assert!(
                bullet.is_some(),
                "case {case:?}: expected a bullet for {name:?} in the ## Errors block, got {:?}",
                module
                    .documented
                    .iter()
                    .map(|bullet| &bullet.name)
                    .collect::<Vec<_>>()
            );
            let bullet = bullet.expect("the bullet asserted above");
            assert!(
                bullet.widened,
                "case {case:?}: expected the bullet to say {WIDENED_MARK:?}, got {bullet:?}"
            );
            assert_eq!(
                bullet.status,
                modelled_status(name),
                "case {case:?}: expected the bullet to state the modelled status {:?}, got {:?}",
                modelled_status(name),
                bullet.status
            );
        }

        // The mark is not decoration: a bullet carrying it is one the attribute widens.
        for bullet in module.documented.iter().filter(|bullet| bullet.widened) {
            let case = format!("{} marks {} widened", module.file, bullet.name);
            assert!(
                module.widened.contains(&bullet.name),
                "case {case:?}: expected extra_errors to name it, got {:?}",
                module.widened
            );
        }
    }
}

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment that OWNS every resource a row creates.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, body)
}

/// The three operations whose widened `ResourceNotFoundException` a request can provoke:
/// the module whose block states it, the collection element of its REST URI, and the
/// payloads a create and an update of that kind carry.
/// `UpdateThemeData$values` is `@required`
/// (`smithy_query(query="shape",shape="UpdateThemeData")`), so the theme patch carries
/// the list it is to hold.
fn update_operations() -> [(&'static str, &'static str, Value, Value); 3] {
    [
        (
            "update_component.rs",
            "components",
            json!({
                "name": "Owned",
                "componentType": "View",
                "properties": {},
                "variants": [],
                "overrides": {},
                "bindingProperties": {},
            }),
            json!({"name": "Ghost"}),
        ),
        (
            "update_form.rs",
            "forms",
            json!({
                "name": "Owned",
                "formActionType": "create",
                "dataType": {"dataSourceType": "DataStore", "dataTypeName": "Post"},
                "fields": {},
                "style": {},
                "sectionalElements": {},
                "schemaVersion": "1.0",
            }),
            json!({"name": "Ghost"}),
        ),
        (
            "update_theme.rs",
            "themes",
            json!({
                "name": "Owned",
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#1d4ed8"}}],
            }),
            json!({
                "name": "Ghost",
                "values": [{"key": "tokens.colors.brand", "value": {"value": "#be123c"}}],
            }),
        ),
    ]
}

/// The forms an id that resolves nowhere can arrive in; each is the same class to the
/// rule, so each must get the status the block states.
fn unresolvable_ids() -> [(&'static str, &'static str); 3] {
    [
        (
            "a well-formed uuid nothing was created with",
            "33333333-3333-4333-8333-333333333333",
        ),
        ("an id of one character", "z"),
        ("an id that is not a uuid at all", "no-such-id"),
    ]
}

/// The loop the stated contract has to close: for each update operation, the status its
/// own `## Errors` block states for the widened `ResourceNotFoundException` is the
/// status the emulator answers for an id that resolves nowhere — and the in-scope update
/// still succeeds.
#[tokio::test]
async fn an_update_answers_the_status_its_block_states_for_the_widened_error() {
    let modules = operation_modules();

    for (file, collection, create_payload, update_payload) in update_operations() {
        let module = modules
            .iter()
            .find(|module| module.file == file)
            .unwrap_or_else(|| panic!("src/ops/{file} must be an operation module of this crate"));
        let bullet = module
            .bullet("ResourceNotFoundException")
            .unwrap_or_else(|| {
                panic!("src/ops/{file} must state ResourceNotFoundException in its ## Errors block")
            });
        assert!(
            bullet.widened,
            "case {file:?}: the block must mark the shape widened, got {bullet:?}"
        );
        let stated = bullet
            .status
            .unwrap_or_else(|| panic!("src/ops/{file} must state the status of the widened error"));

        let router = emulator();
        // A resource DOES exist in the addressed scope, so an empty store is not what
        // decides the answer.
        let collection_uri = format!("/app/{APP_ID}/environment/{ENVIRONMENT}/{collection}");
        let (status, created) =
            call(&router, "POST", &collection_uri, Some(create_payload)).await;
        assert_eq!(
            status, 200,
            "case {file:?}: the create the control row needs must succeed, got {status} with {created:?}"
        );
        let live = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !live.is_empty(),
            "case {file:?}: the create must report an id, got {created:?}"
        );

        for (case, id) in unresolvable_ids() {
            let case = format!("{case} on {file}");
            let (status, body) = call(
                &router,
                "PATCH",
                &format!("{collection_uri}/{id}"),
                Some(update_payload.clone()),
            )
            .await;
            assert_eq!(
                status, stated,
                "case {case:?}: expected the stated HTTP {stated}, got {status} with {body:?}"
            );
        }

        // The control row: the same patch in the scope that owns the id is not a refusal
        // at all, so the stated status is not simply what this operation always answers.
        let (status, body) = call(
            &router,
            "PATCH",
            &format!("{collection_uri}/{live}"),
            Some(update_payload),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {file:?}: the in-scope update must succeed, got {status} with {body:?}"
        );
        assert_eq!(
            body["name"],
            json!("Ghost"),
            "case {file:?}: the in-scope update must report the merged name, got {body:?}"
        );
    }
}
