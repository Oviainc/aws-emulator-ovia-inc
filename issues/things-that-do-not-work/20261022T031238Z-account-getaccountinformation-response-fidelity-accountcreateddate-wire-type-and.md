Issue-Source: trace-sweep-divergence
Issue-Trace: account-account-name-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: account-account-name-lifecycle
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account|GetAccountInformation|2xx->2xx|$.AccountCreatedDate
Issue-Key: account|GetAccountInformation|2xx->2xx|$.AccountName
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# account GetAccountInformation response fidelity: AccountCreatedDate wire type and absent AccountName

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountInformation

## Description
Two causes in the GetAccountInformation response, both on service `account`. Both must be closed before this issue is submitted.

Cause 1 of 2: AccountCreatedDate is serialized as an epoch-seconds number instead of a date-time string.
  AWS: Smithy: GetAccountInformation.AccountCreatedDate targets com.amazonaws.account#AccountCreatedDate, a `timestamp` carrying @timestampFormat("date-time"), so the member MUST be emitted on the wire as an ISO-8601/RFC-3339 date-time STRING (e.g. "2026-10-21T12:34:56.789Z"), not as the restJson1 default epoch-seconds number. Operation is HTTP POST /getAccountInformation, success 200.
  Emulator: the 200 response carries AccountCreatedDate as a JSON number (replay reports the value 1792638609.6452696 where the fixture asserts only the member's SHAPE, `<shaped-not-asserted>`), i.e. a double of epoch seconds. The handler builds `aws_smithy_types::DateTime::from(account.created_date)` and the output serialization emits the default epoch-seconds form, ignoring the member's date-time timestampFormat.
  Why: a client generated from the model expects a date-time string for this member and either fails to deserialize the response or mis-parses the creation instant, so every GetAccountInformation call is unusable for the account-creation date — the number is also not distinguishable as a timestamp by anything reading the JSON directly.
  Fix: the GetAccountInformation output serializer / the output schema's timestamp handling for AccountCreatedDate (make it honour @timestampFormat("date-time") rather than the protocol default).
  Traces: account-account-name-lifecycle#task#2, and account-account-name-lifecycle#task#4 (cluster also covering task#5), where AccountCreatedDate is the ONLY diverging key.

Cause 2 of 2: AccountName is absent from GetAccountInformation for an account that has never been renamed.
  AWS: Smithy: GetAccountInformation output member AccountName (AccountName, @length 1..50, pattern ^[ -;=?-~]+$, @sensitive); the operation is documented as retrieving "information about the specified account including its account name, account ID, account creation date and time, and account state". A real AWS account always has a name, so a first read of an account that the caller has never renamed still reports AccountName — which is exactly why the recorded fixture reads it first, captures it, renames the account, and later restores the original value.
  Emulator: the first GetAccountInformation on the caller's own account returns 200 with AccountName entirely missing (replay: "$.AccountName: missing (expected '{{original_account_name}}', got None)"). The seeded record in the account store is created with `account_name: None` (services/service-com-amazonaws-account-account/src/state.rs, Account::new / Default for ComAmazonawsAccountAccountState), and the handler forwards that Option straight through (services/service-com-amazonaws-account-account/src/ops/get_account_information.rs, `account_name: account.account_name.clone()`), so the member is omitted until some PutAccountName writes one.
  Why: a client that reads the current account name — to display it, or to save and restore it around a rename, as the recorded workflow does — gets nothing back, so a save/restore sequence cannot be expressed and any assertion or capture on the account name fails against the emulator while it succeeds against AWS.
  Fix: the account service state (seed the pre-existing own-account record with an account name satisfying the member's length/pattern constraints) so GetAccountInformation always reports one.
  Traces: account-account-name-lifecycle#task#2.

## Evidence
Fixture account-account-name-lifecycle, step task#2 (GetAccountInformation): expected 200 / actual 200, diverging keys:
  $.AccountCreatedDate: value_mismatch (expected '<shaped-not-asserted>', got 1792638609.6452696) — a JSON number where the model requires a date-time string.
  $.AccountName: missing (expected '{{original_account_name}}', got None).
Fixture account-account-name-lifecycle, step task#4 (cluster also covering task#5): expected 200 / actual 200, $.AccountCreatedDate: value_mismatch (expected '<shaped-not-asserted>', got 1792638609.6452696) — the timestamp-type defect recurs after the rename, where AccountName now matches.
Model references: com.amazonaws.account#AccountCreatedDate = timestamp with timestampFormat "date-time"; GetAccountInformationResponse.AccountName = AccountName (length 1..50, pattern ^[ -;=?-~]+$, sensitive).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture account-account-name-lifecycle, step task#2: POST /getAccountInformation with an empty body (no AccountId, so the caller's own account). Observe 200 with AccountCreatedDate emitted as a JSON number and no AccountName member at all.
3. Continue the fixture: POST /putAccountName with a new AccountName, then step task#4/task#5 POST /getAccountInformation again. AccountName is now present, but AccountCreatedDate is still a JSON number instead of an ISO-8601 date-time string.
