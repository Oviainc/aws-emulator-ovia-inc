// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-managedblockchainquery-tietonchainqueryservice service.
//!
//! ManagedBlockchain Query is a READ-ONLY query service over blockchain data: every
//! modelled operation carries `@readonly` and the model declares no `Create*`, so
//! nothing a caller does can add to the ledger. The ledger therefore has to exist
//! before the first request, and it does — [`Default`] builds a deterministic fixture
//! ledger for every value of the model's `QueryNetwork` enum, and every implemented
//! operation computes its answer by reading that ledger.
//!
//! The real service indexes a PUBLIC chain. This emulator cannot hold one — a public
//! chain's transactions are facts it has no way to compute — so it holds a FIXTURE chain
//! instead, built once by [`chain`] out of fixed labels and the chains' own published
//! facts, spanning each network's life up to the instant the state was constructed.
//! Nothing a request carries ever enters the ledger: an identifier the fixture does not
//! hold is answered with the modelled empty page or `ResourceNotFoundException`, never
//! with a history made up from what was asked for.
//!
//! The fixture is built so the operations are mutually consistent:
//!
//! * a transaction `ListTransactions` returns is fetchable by `GetTransaction`;
//! * its events are exactly what `ListTransactionEvents` returns, and the subset of
//!   them sent to one address is what `ListFilteredTransactionEvents` returns;
//! * whether a Bitcoin output is spent — `TransactionEvent$voutSpent`, which
//!   `ListFilteredTransactionEvents`'s `voutFilter` selects on — is DERIVED from the
//!   inputs that spend it ([`build_ledger`]), so no output can report a spend state the
//!   ledger's own transactions contradict;
//! * every balance `GetTokenBalance`/`ListTokenBalances` reports is DERIVED from the
//!   opening balances plus those events ([`derive_holdings`]) rather than written down
//!   beside them, so a reported balance can never disagree with the transfers that
//!   produced it;
//! * every contract `GetAssetContract` reports is one the network's ledger holds, so an
//!   address no contract is deployed at is a not-found rather than an invented answer.
//!
//! The stored half is [`StoredTransaction`] / [`StoredEvent`] / [`StoredHolding`]; the
//! wire half (`Transaction`, `TransactionOutputItem`, `TransactionEvent`,
//! `TokenBalance`) is PROJECTED from it by the methods below, so no handler assembles a
//! response member by hand.
//!
//! On the MAINNET networks the fixture also holds the published accounts and token
//! contracts a client of a chain index actually reads — [`derived`] writes out their
//! holdings and transfers, and [`well_known_mainnet_erc20`] their contracts — so a
//! balance or a transaction read of a known mainnet account answers out of stored
//! records. An identifier no catalog and no fixture names is still the modelled
//! `ResourceNotFoundException` or the modelled empty page: this emulator indexes no
//! chain, so it reports only what it holds.

pub mod derived;

use std::collections::{BTreeMap, HashMap, HashSet};

use sha2::{Digest, Sha256};

pub mod chain;

use crate::ops::types::{
    BlockchainInstant, ContractMetadata, InternalServerException, OwnerIdentifier, TokenBalance,
    TokenIdentifier, Transaction, TransactionEvent, TransactionOutputItem, ValidationException,
    ValidationExceptionField, QUERY_NETWORK_VALUES,
};

// ---------------------------------------------------------------------------
// Modelled vocabulary
// ---------------------------------------------------------------------------

/// `ConfirmationStatus` values, from the model's own
/// [`crate::ops::types::CONFIRMATION_STATUS_VALUES`].
pub const CONFIRMATION_FINAL: &str = "FINAL";
/// See [`CONFIRMATION_FINAL`].
pub const CONFIRMATION_NONFINAL: &str = "NONFINAL";

/// `ExecutionStatus` values, from the model's own
/// [`crate::ops::types::EXECUTION_STATUS_VALUES`].
pub const EXECUTION_SUCCEEDED: &str = "SUCCEEDED";
/// See [`EXECUTION_SUCCEEDED`].
pub const EXECUTION_FAILED: &str = "FAILED";

/// `SortOrder` values, from the model's own [`crate::ops::types::SORT_ORDER_VALUES`].
pub const SORT_ASCENDING: &str = "ASCENDING";
/// See [`SORT_ASCENDING`].
pub const SORT_DESCENDING: &str = "DESCENDING";

/// The `tokenId` of Ether, the native token of the Ethereum networks.
///
/// `TokenIdentifier$tokenId`'s documentation: "For native tokens, use the 3 character
/// abbreviation that best matches your token. For example, `btc` for Bitcoin, `eth` for
/// Ether".
pub const NATIVE_TOKEN_ETH: &str = "eth";
/// The `tokenId` of Bitcoin; see [`NATIVE_TOKEN_ETH`] for where the spelling comes from.
pub const NATIVE_TOKEN_BTC: &str = "btc";

/// The only value the model's `ResourceType` enum declares, so it is the
/// `resourceType` every `ResourceNotFoundException` this service raises carries.
pub const RESOURCE_TYPE: &str = "collection";

/// The `ValidationExceptionReason` for a request member whose value this service
/// refuses, from the model's own `VALIDATION_EXCEPTION_REASON_VALUES`.
pub const VALIDATION_REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `ErrorType` a batch read reports a refused request item as ("An API request
/// validation exception"), from the model's own
/// [`crate::ops::types::ERROR_TYPE_VALUES`].
pub const ERROR_TYPE_VALIDATION_EXCEPTION: &str = "VALIDATION_EXCEPTION";
/// The `ErrorType` a batch read reports an item it found nothing for as ("An API
/// request retrieving an item that can't be found"); see
/// [`ERROR_TYPE_VALIDATION_EXCEPTION`].
pub const ERROR_TYPE_RESOURCE_NOT_FOUND_EXCEPTION: &str = "RESOURCE_NOT_FOUND_EXCEPTION";

/// The largest page any listing returns, and the page size used when the caller names
/// no `maxResults`.
///
/// Both halves of the model agree on this number: `maxResults` carries
/// `@range(min: 1, max: 250)` on every paginated operation, and each output list
/// carries `@length(min: 0, max: 250)`.
pub const MAX_PAGE_SIZE: i32 = 250;

/// The networks this service calls Bitcoin networks.
///
/// Spelled out rather than derived from the `BITCOIN_` prefix because the model states
/// the set in two places and both name exactly these two values:
/// `QueryNetwork`'s value list (`BITCOIN_MAINNET | BITCOIN_TESTNET | ETHEREUM_MAINNET |
/// ETHEREUM_SEPOLIA_TESTNET`) and `ListFilteredTransactionEventsInput$network`'s
/// documentation ("Valid Values: BITCOIN_MAINNET | BITCOIN_TESTNET").
pub const BITCOIN_NETWORKS: [&str; 2] = ["BITCOIN_MAINNET", "BITCOIN_TESTNET"];

/// Whether `network` is one of the model's two Bitcoin networks.
///
/// The Bitcoin and Ethereum networks differ in what a transaction carries — a Bitcoin
/// transaction has a `transactionId` and no gas, an Ethereum one has gas and a
/// signature — so the fixture builds the two kinds differently.
pub fn is_bitcoin(network: &str) -> bool {
    BITCOIN_NETWORKS.contains(&network)
}

// ---------------------------------------------------------------------------
// Stored records
// ---------------------------------------------------------------------------

/// A transaction's position in the ledger: `(transactionTimestamp, transactionIndex,
/// transactionHash)`.
///
/// Ordered first by timestamp and then by the index within the block, so iterating the
/// `BTreeMap` keyed by it walks the chain in the order it was written — which is the
/// order `ListTransactions` sorts by (`ListTransactionsSortBy` declares exactly one
/// value, `TRANSACTION_TIMESTAMP`). The hash is the tie-breaker that makes the key
/// total.
pub type TxKey = (i64, i64, String);

/// A stored transaction: the state `Transaction` and `TransactionOutputItem` are
/// rebuilt from.
#[derive(Debug, Clone)]
pub struct StoredTransaction {
    pub network: String,
    /// The hash as the ledger spells it: `0x`-prefixed on the Ethereum networks, bare
    /// hex on the Bitcoin ones. Lookups go through [`normalize_hash`], so a caller may
    /// spell it either way.
    pub transaction_hash: String,
    /// Set only on the Bitcoin networks — `GetTransactionInput$transactionId`'s
    /// documentation: "transactionId is only supported on the Bitcoin networks".
    pub transaction_id: Option<String>,
    pub block_hash: Option<String>,
    pub block_number: Option<String>,
    /// Seconds since the Unix epoch; `BlockchainInstant$time` is "only be recorded up
    /// to the second".
    pub transaction_timestamp: i64,
    pub transaction_index: i64,
    pub number_of_transactions: i64,
    pub to: String,
    pub from: Option<String>,
    pub contract_address: Option<String>,
    pub gas_used: Option<i128>,
    pub cumulative_gas_used: Option<i128>,
    pub effective_gas_price: Option<i128>,
    pub transaction_fee: Option<i128>,
    pub signature_v: Option<i32>,
    pub signature_r: Option<String>,
    pub signature_s: Option<String>,
    /// One of [`CONFIRMATION_FINAL`] / [`CONFIRMATION_NONFINAL`].
    pub confirmation_status: String,
    /// One of [`EXECUTION_SUCCEEDED`] / [`EXECUTION_FAILED`]; absent on the Bitcoin
    /// networks, which have no contract execution to succeed or fail.
    pub execution_status: Option<String>,
    /// The events this transaction emitted, in emission order — which is the order
    /// `ListTransactionEvents` returns them in.
    pub events: Vec<StoredEvent>,
}

impl StoredTransaction {
    /// This transaction's key in the ledger.
    pub fn key(&self) -> TxKey {
        (
            self.transaction_timestamp,
            self.transaction_index,
            normalize_hash(&self.transaction_hash),
        )
    }

    /// Project the stored transaction onto the modelled `Transaction`.
    pub fn transaction(&self) -> Transaction {
        Transaction {
            network: self.network.clone(),
            block_hash: self.block_hash.clone(),
            transaction_hash: self.transaction_hash.clone(),
            block_number: self.block_number.clone(),
            transaction_timestamp: instant_secs(self.transaction_timestamp),
            transaction_index: self.transaction_index,
            number_of_transactions: self.number_of_transactions,
            to: self.to.clone(),
            from: self.from.clone(),
            contract_address: self.contract_address.clone(),
            gas_used: self.gas_used.map(|amount| amount.to_string()),
            cumulative_gas_used: self.cumulative_gas_used.map(|amount| amount.to_string()),
            effective_gas_price: self.effective_gas_price.map(|amount| amount.to_string()),
            signature_v: self.signature_v,
            signature_r: self.signature_r.clone(),
            signature_s: self.signature_s.clone(),
            transaction_fee: self.transaction_fee.map(|amount| amount.to_string()),
            transaction_id: self.transaction_id.clone(),
            confirmation_status: Some(self.confirmation_status.clone()),
            execution_status: self.execution_status.clone(),
        }
    }

    /// Project the stored transaction onto the modelled `TransactionOutputItem`, the
    /// summary `ListTransactions` returns.
    pub fn output_item(&self) -> TransactionOutputItem {
        TransactionOutputItem {
            transaction_hash: self.transaction_hash.clone(),
            transaction_id: self.transaction_id.clone(),
            network: self.network.clone(),
            transaction_timestamp: instant_secs(self.transaction_timestamp),
            confirmation_status: Some(self.confirmation_status.clone()),
        }
    }

    /// Project the stored events onto the modelled `TransactionEvent`s.
    ///
    /// `network`, `transactionHash`, `transactionId`, `blockchainInstant` and
    /// `confirmationStatus` are properties of the TRANSACTION, so they come from it
    /// rather than being stored a second time on each event.
    pub fn events(&self) -> Vec<TransactionEvent> {
        self.events
            .iter()
            .map(|event| TransactionEvent {
                network: self.network.clone(),
                transaction_hash: self.transaction_hash.clone(),
                event_type: event.event_type.clone(),
                from: event.from.clone(),
                to: event.to.clone(),
                value: event.value.map(|amount| amount.to_string()),
                contract_address: event.contract_address.clone(),
                token_id: event.token_id.clone(),
                transaction_id: self.transaction_id.clone(),
                vout_index: event.vout_index,
                vout_spent: event.vout_spent,
                spent_vout_transaction_id: event.spent_vout_transaction_id.clone(),
                spent_vout_transaction_hash: event.spent_vout_transaction_hash.clone(),
                spent_vout_index: event.spent_vout_index,
                blockchain_instant: Some(BlockchainInstant {
                    time: Some(instant_secs(self.transaction_timestamp)),
                }),
                confirmation_status: Some(self.confirmation_status.clone()),
            })
            .collect()
    }

    /// Every address this transaction involves: its own `from`/`to`, plus the `from`
    /// and `to` of each event it emitted.
    ///
    /// The events matter: a token transfer's `to` is the token CONTRACT, and the wallet
    /// that received the tokens appears only on the transfer event, so an address-scoped
    /// listing that looked at the transaction alone would miss it.
    pub fn addresses(&self) -> Vec<&str> {
        let mut addresses = vec![self.to.as_str()];
        if let Some(from) = &self.from {
            addresses.push(from.as_str());
        }
        for event in &self.events {
            if let Some(from) = &event.from {
                addresses.push(from.as_str());
            }
            if let Some(to) = &event.to {
                addresses.push(to.as_str());
            }
        }
        addresses.sort_unstable();
        addresses.dedup();
        addresses
    }
}

/// A stored transaction event: the state `TransactionEvent` is rebuilt from, holding
/// only what belongs to the EVENT (see [`StoredTransaction::events`]).
#[derive(Debug, Clone, Default)]
pub struct StoredEvent {
    /// One of the model's `QueryTransactionEventType` values, listed in this crate as
    /// [`crate::ops::types::QUERY_TRANSACTION_EVENT_TYPE_VALUES`].
    pub event_type: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub value: Option<i128>,
    pub contract_address: Option<String>,
    pub token_id: Option<String>,
    pub vout_index: Option<i32>,
    pub vout_spent: Option<bool>,
    pub spent_vout_transaction_id: Option<String>,
    pub spent_vout_transaction_hash: Option<String>,
    pub spent_vout_index: Option<i32>,
}

impl StoredEvent {
    /// The token whose balance this event moves, as the `(contractAddress, tokenId)`
    /// pair a [`HoldingKey`] is built from — or `None` for an event that moves no
    /// balance.
    ///
    /// Which pair identifies the token follows from the event's type, the way the token
    /// standards define it: a native transfer moves the network's native token (no
    /// contract, the documented 3-character abbreviation as `tokenId`), an ERC-20 event
    /// moves one fungible balance per contract (a contract and no `tokenId`), and an
    /// ERC-721/ERC-1155 event moves one token of a contract (a contract AND a
    /// `tokenId`).
    pub fn holding(&self) -> Option<(Option<String>, Option<String>)> {
        match self.event_type.as_str() {
            "ETH_TRANSFER" | "INTERNAL_ETH_TRANSFER" => {
                Some((None, Some(NATIVE_TOKEN_ETH.to_string())))
            }
            "BITCOIN_VIN" | "BITCOIN_VOUT" => Some((None, Some(NATIVE_TOKEN_BTC.to_string()))),
            kind if kind.starts_with("ERC20_") => self
                .contract_address
                .clone()
                .map(|contract| (Some(contract), None)),
            "ERC721_TRANSFER" | "ERC1155_TRANSFER" => {
                match (&self.contract_address, &self.token_id) {
                    (Some(contract), Some(token_id)) => {
                        Some((Some(contract.clone()), Some(token_id.clone())))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// What identifies one holding: the token (as a `TokenIdentifier` names it) and its
/// owner.
///
/// `Ord` is derived so the `BTreeMap` keyed by it iterates contract-major, which is the
/// stable order `ListTokenBalances` pages over.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HoldingKey {
    /// Absent for a native token (BTC, ETH), which has no contract.
    pub contract_address: Option<String>,
    /// Absent for an ERC-20 balance, which is one fungible balance per contract.
    pub token_id: Option<String>,
    pub owner_address: String,
}

impl HoldingKey {
    /// Whether a `TokenFilter`/`TokenIdentifier` naming `contract_address` and
    /// `token_id` selects this holding: a member the filter does not name matches
    /// anything, and one it does name must match exactly.
    pub fn matches_token(&self, contract_address: Option<&str>, token_id: Option<&str>) -> bool {
        contract_address.is_none_or(|wanted| self.contract_address.as_deref() == Some(wanted))
            && token_id.is_none_or(|wanted| self.token_id.as_deref() == Some(wanted))
    }

    /// Project the token half of this key onto the modelled `TokenIdentifier`.
    pub fn token_identifier(&self, network: &str) -> TokenIdentifier {
        TokenIdentifier {
            network: network.to_string(),
            contract_address: self.contract_address.clone(),
            token_id: self.token_id.clone(),
        }
    }

    /// This key as the ledger spells it.
    ///
    /// Only the ADDRESSES are touched, and only where their spelling carries no
    /// information — see [`derived::normalize_address`]. A lookup goes through this so
    /// the two spellings of one Ethereum account read the same balance.
    pub fn normalized(&self) -> HoldingKey {
        HoldingKey {
            contract_address: self
                .contract_address
                .as_deref()
                .map(derived::normalize_address),
            token_id: self.token_id.clone(),
            owner_address: derived::normalize_address(&self.owner_address),
        }
    }

    /// Project the owner half of this key onto the modelled `OwnerIdentifier`.
    pub fn owner_identifier(&self) -> OwnerIdentifier {
        OwnerIdentifier {
            address: self.owner_address.clone(),
        }
    }

    /// The `resourceId` an error about this holding reports, and the key
    /// `ListTokenBalances` pages over.
    ///
    /// Unambiguous despite joining three members: `ChainAddress` is
    /// `^[-A-Za-z0-9]{13,74}$` and `QueryTokenId` is `^[a-zA-Z0-9]{1,66}$`, so no member
    /// can contain the `/` separator, and neither can equal the `-` that stands in for
    /// an absent one (an address is at least 13 characters, and a `tokenId` admits no
    /// `-` at all).
    pub fn resource_id(&self) -> String {
        format!(
            "{}/{}/{}",
            self.contract_address.as_deref().unwrap_or("-"),
            self.token_id.as_deref().unwrap_or("-"),
            self.owner_address
        )
    }
}

/// One balance the ledger recorded, and when.
#[derive(Debug, Clone, Copy)]
pub struct BalanceAt {
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub balance: i128,
}

/// A stored holding: the balance history of one (token, owner) pair, ascending by time.
///
/// A history rather than a single number because `GetTokenBalance` takes an
/// `atBlockchainInstant`, and a point-in-time balance cannot be answered from the
/// current one.
#[derive(Debug, Clone, Default)]
pub struct StoredHolding {
    pub history: Vec<BalanceAt>,
}

impl StoredHolding {
    /// Record `balance` as the holding's balance from `time` on. A second record at the
    /// same instant replaces the first, so several events in one block settle to the
    /// balance the last of them left.
    pub fn record(&mut self, time: i64, balance: i128) {
        match self.history.last_mut() {
            Some(last) if last.time == time => last.balance = balance,
            _ => self.history.push(BalanceAt { time, balance }),
        }
    }

    /// The balance as of `at` — the last record at or before that instant — or the
    /// current balance when no instant is asked for.
    ///
    /// `None` when the ledger knows no balance for this holding at that instant, which
    /// is what an owner who had not yet received the token looks like.
    pub fn balance_at(&self, at: Option<i64>) -> Option<BalanceAt> {
        match at {
            None => self.history.last().copied(),
            Some(instant) => self
                .history
                .iter()
                .rev()
                .find(|record| record.time <= instant)
                .copied(),
        }
    }

    /// Project this holding onto the modelled `TokenBalance`.
    ///
    /// `atBlockchainInstant` is the instant the balance was ASKED for —
    /// `TokenBalance$atBlockchainInstant` is "the time for when the TokenBalance is
    /// requested or the current time if a time is not provided in the request" — while
    /// `lastUpdatedTime` is "the `Timestamp` of the last transaction at which the
    /// balance for the token in the wallet was updated", so it is the time of the
    /// record the balance was read from.
    pub fn token_balance(
        &self,
        network: &str,
        key: &HoldingKey,
        asked_at: i64,
        record: BalanceAt,
    ) -> TokenBalance {
        TokenBalance {
            owner_identifier: Some(key.owner_identifier()),
            token_identifier: Some(key.token_identifier(network)),
            balance: record.balance.to_string(),
            at_blockchain_instant: BlockchainInstant {
                time: Some(instant_secs(asked_at)),
            },
            last_updated_time: Some(BlockchainInstant {
                time: Some(instant_secs(record.time)),
            }),
        }
    }
}

/// A stored asset contract: the state `GetAssetContract` answers from, and the state the
/// modelled `AssetContract` is projected from.
#[derive(Debug, Clone)]
pub struct StoredContract {
    pub network: String,
    /// The address as the ledger spells it. Lookups go through
    /// [`normalize_contract_address`], so a caller may spell it either way.
    pub contract_address: String,
    /// One of the model's `QueryTokenStandard` values, listed in this crate as
    /// [`crate::ops::types::QUERY_TOKEN_STANDARD_VALUES`].
    pub token_standard: String,
    pub deployer_address: String,
    pub metadata: Option<ContractMetadata>,
}

/// One network's ledger.
#[derive(Debug, Default)]
pub struct NetworkLedger {
    /// Which value of the model's `QueryNetwork` enum this ledger holds, so the fixture
    /// [`chain`] builds for it takes the shape that network uses (a Bitcoin transaction
    /// has a `transactionId` and no gas, an Ethereum one the reverse).
    pub network: String,
    /// Every transaction on this network, keyed by [`TxKey`] so iteration is in chain
    /// order.
    pub transactions: BTreeMap<TxKey, StoredTransaction>,
    /// `transactionHash` (normalised) -> the key in `transactions`.
    by_hash: HashMap<String, TxKey>,
    /// `transactionId` (normalised) -> the key in `transactions`.
    by_id: HashMap<String, TxKey>,
    /// Every address that appears on a transaction -> that transaction's key, in chain
    /// order.
    by_address: HashMap<String, Vec<TxKey>>,
    /// Every holding on this network, derived from the transactions by
    /// [`derive_holdings`].
    pub holdings: BTreeMap<HoldingKey, StoredHolding>,
    /// The balance each holding OPENED with, and when: the base the transactions are
    /// replayed onto. Kept rather than consumed, so every balance is re-derivable from
    /// the same base and can never report one that disagrees with the events it holds.
    opening: Vec<(HoldingKey, BalanceAt)>,
    /// Every asset contract on this network, keyed by its [`normalize_contract_address`]
    /// form so one address resolves however it is spelled; iteration is therefore in a
    /// stable, address-major order.
    pub contracts: BTreeMap<String, StoredContract>,
}

impl NetworkLedger {
    /// Store `transaction` and index it by hash, id and participating address.
    ///
    /// The only way a transaction enters a ledger, so the indexes cannot drift from the
    /// `transactions` map.
    pub fn insert_transaction(&mut self, transaction: StoredTransaction) {
        let key = transaction.key();
        self.by_hash
            .insert(normalize_hash(&transaction.transaction_hash), key.clone());
        if let Some(id) = &transaction.transaction_id {
            self.by_id.insert(normalize_hash(id), key.clone());
        }
        for address in transaction.addresses() {
            // Keyed by the spelling an address is looked up under
            // ([`derived::normalize_address`]), so one account has one entry however the
            // transaction spelled it.
            let keys = self
                .by_address
                .entry(derived::normalize_address(address))
                .or_default();
            keys.push(key.clone());
            // Chain order, which is the order `ListTransactions` reports them in.
            keys.sort();
            keys.dedup();
        }
        self.transactions.insert(key, transaction);
    }

    /// The transaction with this `transactionHash`, however the caller spelled it.
    pub fn by_hash(&self, hash: &str) -> Option<&StoredTransaction> {
        let key = self.by_hash.get(&normalize_hash(hash))?;
        self.transactions.get(key)
    }

    /// The transaction with this `transactionId`, however the caller spelled it.
    pub fn by_id(&self, id: &str) -> Option<&StoredTransaction> {
        let key = self.by_id.get(&normalize_hash(id))?;
        self.transactions.get(key)
    }

    /// The asset contract deployed at `address`, however the caller spelled it.
    pub fn contract(&self, address: &str) -> Option<&StoredContract> {
        self.contracts.get(&normalize_contract_address(address))
    }

    /// Every transaction `address` took part in, in chain order — however the caller
    /// spelled the address ([`derived::normalize_address`]).
    pub fn for_address(&self, address: &str) -> Vec<&StoredTransaction> {
        self.by_address
            .get(&derived::normalize_address(address))
            .into_iter()
            .flatten()
            .filter_map(|key| self.transactions.get(key))
            .collect()
    }

    /// Record the balance a holding opens with, unless the ledger already has one.
    ///
    /// An opening balance is what an address held before the first transaction this
    /// ledger holds for it, so a holding has at most one — recording a second would put
    /// the balance history out of order.
    fn record_opening(&mut self, key: HoldingKey, time: i64, balance: i128) {
        if self.opening.iter().any(|(existing, _)| *existing == key) {
            return;
        }
        self.opening.push((key, BalanceAt { time, balance }));
    }

    /// Re-derive every balance from the opening balances and the stored transactions.
    ///
    /// Balances are never written beside the transfers that produced them, they are
    /// REPLAYED from them ([`derive_holdings`]).
    fn recompute_holdings(&mut self) {
        self.holdings = derive_holdings(&self.transactions, &self.opening);
    }
}

/// Per-service state for com-amazonaws-managedblockchainquery-tietonchainqueryservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState {
    /// One ledger per value of the model's `QueryNetwork` enum.
    pub networks: BTreeMap<String, NetworkLedger>,
}

/// The ledger the emulator serves. There is no write API, so this is the only thing
/// that ever populates the store.
impl Default for ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState {
    fn default() -> Self {
        Self {
            networks: seed_networks(),
        }
    }
}

impl ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState {
    /// The ledger for `network`, or `None` for a network this emulator holds none for.
    ///
    /// The boundary validator has already refused any value the `QueryNetwork` enum
    /// does not declare by the time a handler runs, so in practice every network
    /// resolves.
    pub fn ledger(&self, network: &str) -> Option<&NetworkLedger> {
        self.networks.get(network)
    }

    /// The balance of one (token, owner) pair as of the instant the caller asked for,
    /// projected onto the modelled `TokenBalance`.
    ///
    /// The ONE place a per-holding balance read is decided, so `GetTokenBalance` and
    /// every item of a `BatchGetTokenBalance` answer the same question the same way —
    /// the single read raises the [`BalanceFailure`] as the call's modelled error, while
    /// the batch read reports it as one `BatchGetTokenBalanceErrorItem` and still
    /// answers the rest of the request.
    ///
    /// Named apart from [`StoredHolding::token_balance`], the projection it finishes
    /// with: this is the READ (resolve the key, pick the record in force, refuse what
    /// cannot be answered), that one only renders a record on the wire.
    pub fn read_token_balance(
        &self,
        token: &TokenIdentifier,
        owner: &OwnerIdentifier,
        at_instant: Option<&BlockchainInstant>,
    ) -> Result<TokenBalance, BalanceFailure> {
        // `TokenIdentifier` is "the container for the identifier for the token including
        // the unique token ID and its blockchain network": a network on its own names no
        // token, so one of `contractAddress`/`tokenId` has to be there. Both are
        // modelled optional, so the validator cannot say this.
        if token.contract_address.is_none() && token.token_id.is_none() {
            return Err(BalanceFailure::Unidentified(field_validation_exception(
                "tokenIdentifier",
                "tokenIdentifier must specify one of contractAddress or tokenId",
            )));
        }

        // The key as the LEDGER spells it, so the two spellings of one Ethereum account
        // or token contract read one balance (see [`derived::normalize_address`]).
        let key = HoldingKey {
            contract_address: token.contract_address.clone(),
            token_id: token.token_id.clone(),
            owner_address: owner.address.clone(),
        }
        .normalized();
        let at = instant_to_secs(at_instant);

        let holding = self
            .ledger(&token.network)
            .and_then(|ledger| ledger.holdings.get(&key));
        let record = holding.and_then(|holding| holding.balance_at(at));
        let (holding, record) = match (holding, record) {
            (Some(holding), Some(record)) => (holding, record),
            _ => {
                return Err(BalanceFailure::NotFound(not_found(key.resource_id())));
            }
        };

        // `TokenBalance$atBlockchainInstant`: "the time for when the TokenBalance is
        // requested or the current time if a time is not provided in the request".
        let asked_at = at.unwrap_or_else(now_secs);
        Ok(holding.token_balance(&token.network, &key, asked_at, record))
    }
}

/// Why the ledger could not report a (token, owner) balance.
///
/// Carries the modelled exception the failure IS, so the single-item read answers with
/// it directly and the batch read renders the same case as an error item: the
/// `errorType` is the modelled `ErrorType` for that case, the `errorCode` the name of
/// the exception's shape, and the `errorMessage` the exception's own message.
#[derive(Debug)]
pub enum BalanceFailure {
    /// The `tokenIdentifier` names a network but no token.
    Unidentified(ValidationException),
    /// The ledger knows no balance for that (token, owner) at the requested instant.
    NotFound(crate::ops::types::ResourceNotFoundException),
}

impl BalanceFailure {
    /// The wire code of the modelled exception this failure is: the name of its shape,
    /// which is the code that exception reports when it is raised as a call's error.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Unidentified(_) => "ValidationException",
            Self::NotFound(_) => "ResourceNotFoundException",
        }
    }

    /// The modelled `ErrorType` of this failure.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::Unidentified(_) => ERROR_TYPE_VALIDATION_EXCEPTION,
            Self::NotFound(_) => ERROR_TYPE_RESOURCE_NOT_FOUND_EXCEPTION,
        }
    }

    /// The exception's own message, so a batch error item says exactly what the
    /// single-item read's error would have said.
    pub fn message(&self) -> &str {
        match self {
            Self::Unidentified(error) => &error.message,
            Self::NotFound(error) => &error.message,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ComAmazonawsManagedblockchainqueryTietonchainqueryserviceState,
        >)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// A `BlockchainInstant`-grade timestamp from whole seconds.
///
/// `BlockchainInstant$time`'s documentation: "This `timestamp` will only be recorded up
/// to the second", so every instant this service reports is whole seconds.
pub fn instant_secs(secs: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(secs)
}

/// The whole seconds of a `BlockchainInstant` a request carried, if it carried a time.
pub fn instant_to_secs(instant: Option<&BlockchainInstant>) -> Option<i64> {
    instant.and_then(|instant| instant.time).map(|time| time.secs())
}

/// Now, in whole seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

/// A transaction hash or id reduced to the form the ledger is keyed by.
///
/// The modelled `QueryTransactionHash`/`QueryTransactionId` pattern
/// `^(0x[A-Fa-f0-9]{64}|[A-Fa-f0-9]{64})$` admits the same 64 hex digits with or without
/// the `0x` prefix and in either letter case, so every one of those spellings names ONE
/// transaction and a lookup has to see them as equal.
pub fn normalize_hash(value: &str) -> String {
    value
        .strip_prefix("0x")
        .unwrap_or(value)
        .to_ascii_lowercase()
}

/// A contract address reduced to the form the ledger keys contracts by.
///
/// An asset contract lives on an EVM network (`GetAssetContract`'s documentation: "The
/// Bitcoin blockchain networks do not support this operation"), where an address is 20
/// bytes: the mixed case of an EIP-55 checksummed spelling and the `0x` prefix are
/// presentation and not identity, and `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` admits
/// every one of those spellings, so a lookup has to see them as one contract.
pub fn normalize_contract_address(value: &str) -> String {
    let lowercase = value.to_ascii_lowercase();
    lowercase
        .strip_prefix("0x")
        .unwrap_or(&lowercase)
        .to_string()
}

/// One page of a listing: the items on it, and the token that resumes after them.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` this service did not issue, or that names no position in the listing
/// being paged. Each handler renders it as its own modelled `ValidationException`.
#[derive(Debug)]
pub struct InvalidPageToken(pub String);

/// How a request names one transaction: by `transactionHash` or by `transactionId`.
///
/// Shared by `GetTransaction` and `ListTransactionEvents`, which model the same pair of
/// members and so accept the same combinations.
#[derive(Debug)]
pub enum TransactionSelector {
    Hash(String),
    Id(String),
}

impl TransactionSelector {
    /// Which transaction a request's `transactionHash`/`transactionId` name, refusing
    /// the combinations this service does not accept.
    ///
    /// Both members are modelled OPTIONAL, so the boundary validator lets through a
    /// request naming NEITHER or BOTH, while a transaction-scoped read needs exactly
    /// one. And `transactionId` is a Bitcoin identifier — the member's own
    /// documentation: "transactionId is only supported on the Bitcoin networks" — so it
    /// is refused on any other network.
    ///
    /// A refusal carries one `fieldList` entry per rule the request broke, under the
    /// generic top-level message every such refusal shares
    /// ([`validation_exception`]): a request that breaks both rules at once (both
    /// members, on a network that admits no `transactionId`) reports both.
    pub fn from_input(
        network: &str,
        hash: Option<&str>,
        id: Option<&str>,
    ) -> Result<Self, ValidationException> {
        // One entry per violated rule, in the order the rules are stated: which member
        // names the transaction first, then whether this network admits the member that
        // was used. A request can break both at once (both members on an EVM network),
        // and `fieldList` is the half of the error a client switches on, so each
        // violation contributes its own entry rather than being collapsed into prose.
        let mut fields = Vec::new();
        let selector = match (hash, id) {
            (Some(hash), None) => Some(Self::Hash(hash.to_string())),
            (None, Some(id)) => Some(Self::Id(id.to_string())),
            (Some(_), Some(_)) => {
                fields.push(validation_field(
                    "transactionHash",
                    "The request must include either a transactionId or transactionHash \
                     but not both.",
                ));
                None
            }
            (None, None) => {
                fields.push(validation_field(
                    "transactionHash",
                    "The request must include a transactionHash.",
                ));
                None
            }
        };
        if id.is_some() && !is_bitcoin(network) {
            fields.push(validation_field(
                "transactionId",
                "transactionId can only be specified for Bitcoin networks.",
            ));
        }
        match selector {
            Some(selector) if fields.is_empty() => Ok(selector),
            _ => Err(validation_exception(fields)),
        }
    }

    /// The identifier as the request spelled it, for an error's `resourceId`.
    pub fn identifier(&self) -> &str {
        match self {
            Self::Hash(hash) => hash,
            Self::Id(id) => id,
        }
    }

    /// Which member the identifier came from, for a `fieldList` entry about it.
    pub fn member(&self) -> &'static str {
        match self {
            Self::Hash(_) => "transactionHash",
            Self::Id(_) => "transactionId",
        }
    }

    /// The transaction it names, or `None` when the ledger holds none.
    pub fn resolve<'ledger>(
        &self,
        ledger: &'ledger NetworkLedger,
    ) -> Option<&'ledger StoredTransaction> {
        match self {
            Self::Hash(hash) => ledger.by_hash(hash),
            Self::Id(id) => ledger.by_id(id),
        }
    }
}

/// Take one page out of an ordered listing.
///
/// `all` is every item that matches the request, in the order the response reports
/// them, paired with the key that identifies the item within the listing. The page holds
/// at most `max_results` items and carries a token only when further items remain, so a
/// caller that follows tokens to exhaustion sees every item exactly once. The token
/// names the first item NOT on the page, which is why resuming is an exact key match
/// and works for either `sortOrder`.
pub fn page<T>(
    all: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, InvalidPageToken> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let key =
                decode_page_token(token).ok_or_else(|| InvalidPageToken(token.to_string()))?;
            all.iter()
                .position(|(candidate, _)| *candidate == key)
                .ok_or_else(|| InvalidPageToken(token.to_string()))?
        }
    };
    // `maxResults` carries `@range(min: 1, max: 250)`, so the boundary validator has
    // already refused anything outside it; absent, the page is the modelled maximum.
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;

    let mut rest = all.into_iter().skip(start);
    let items: Vec<T> = rest.by_ref().take(limit).map(|(_, item)| item).collect();
    let next_token = rest.next().map(|(key, _)| encode_page_token(&key));
    Ok(Page { items, next_token })
}

/// Marks a continuation token as this service's, and at which version it was issued.
const PAGE_TOKEN_PREFIX: &str = "managedblockchain-query:v1:";

/// Encode a listing key as an opaque continuation token.
///
/// The key is prefixed with a version marker before hex-encoding, so the token is
/// opaque to a client AND a value this service did not issue is refused rather than
/// resumed from. It stays a pure function of the key, so no cursor state is held
/// server-side — which matters for a read-only service whose reads must not write.
fn encode_page_token(key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_PREFIX}{key}"))
}

/// The inverse of [`encode_page_token`]; `None` for anything this service did not issue.
fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// The top-level `message` every `ValidationException` this service raises for a request
/// it refuses on its own rules.
///
/// It is GENERIC on purpose: the exception models a `fieldList` of
/// `ValidationExceptionField{name, message}` as "the container for the `fieldList` of the
/// exception", so the per-rule detail belongs there, where a client can branch on it,
/// and the top-level message stays the one sentence that is true of every such refusal.
/// Request detail is therefore never interpolated into it.
pub const INVALID_PARAMETER_VALUES_MESSAGE: &str = "One or more parameter values were invalid.";

/// One `fieldList` entry: the member a rule was violated on, and what that rule says.
///
/// `ValidationExceptionField$name` carries `@required` in the model, so every entry
/// names a member — the member the rule is stated about when the rule spans a pair of
/// them (`transactionHash`/`transactionId`).
pub fn validation_field(name: &str, message: impl Into<String>) -> ValidationExceptionField {
    ValidationExceptionField {
        name: name.to_string(),
        message: message.into(),
    }
}

/// This service's modelled `ValidationException` over one entry per violated rule.
///
/// `message` is [`INVALID_PARAMETER_VALUES_MESSAGE`] and `reason` is the model's own
/// `fieldValidationFailed`; `fields` is what a client reads to learn what to fix.
pub fn validation_exception(fields: Vec<ValidationExceptionField>) -> ValidationException {
    ValidationException {
        message: INVALID_PARAMETER_VALUES_MESSAGE.to_string(),
        reason: VALIDATION_REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(fields),
    }
}

/// This service's modelled `ValidationException` for ONE request member it refuses.
pub fn field_validation_exception(name: &str, message: impl Into<String>) -> ValidationException {
    validation_exception(vec![validation_field(name, message)])
}

/// Render a refused `nextToken` as this service's modelled `ValidationException`.
///
/// The token appears in the `fieldList` entry, never in the top-level message: the entry
/// is the per-member detail a client reads to fix the request, which is exactly where
/// request-derived text belongs.
pub fn invalid_page_token(error: &InvalidPageToken) -> ValidationException {
    field_validation_exception(
        "nextToken",
        format!("'{}' is not a valid pagination token", error.0),
    )
}

/// The fixed `message` every `ResourceNotFoundException` this service raises carries.
///
/// Request-independent for the same reason as [`INVALID_PARAMETER_VALUES_MESSAGE`]: the
/// exception models `resourceId` and `resourceType` as the containers for "the resource
/// that caused the exception", so the identity travels in those members and not in the
/// message.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// This service's modelled `ResourceNotFoundException` for a resource the ledger does
/// not hold.
///
/// `resourceType` is [`RESOURCE_TYPE`] because that is the only value the model's
/// `ResourceType` enum declares.
pub fn not_found(resource_id: impl Into<String>) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: resource_id.into(),
        resource_type: RESOURCE_TYPE.to_string(),
    }
}

/// The modelled server error every handler answers with when [`service_state`] finds no
/// store (this service mounted on an engine that holds none).
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the managedblockchain-query ledger is unavailable".to_string(),
        // Nothing about a missing store resolves with time, so no retry hint.
        retry_after_seconds: None,
    }
}

// ---------------------------------------------------------------------------
// The fixture ledger
// ---------------------------------------------------------------------------

/// One Ether in wei, the unit every native Ethereum balance is held in.
const ETH: i128 = 1_000_000_000_000_000_000;

/// One Bitcoin in satoshi, the unit every native Bitcoin balance is held in.
const BTC: i128 = 100_000_000;

/// The gas price every fixture Ethereum transaction paid, in wei (20 gwei).
const EFFECTIVE_GAS_PRICE: i128 = 20_000_000_000;

/// One transaction of the fixture, before the ledger-wide values (timestamps, block
/// identity, cumulative gas, fees) are computed for it.
struct SeedTx {
    hash: String,
    transaction_id: Option<String>,
    /// Which fixture block this transaction is in, counting from 0 at the instant the
    /// fixture's chain starts ([`chain`]).
    block: i64,
    from: Option<String>,
    to: String,
    contract_address: Option<String>,
    gas_used: Option<i128>,
    confirmation_status: &'static str,
    execution_status: Option<&'static str>,
    events: Vec<StoredEvent>,
}

/// The fixture ledger, one per value of the model's `QueryNetwork` enum.
///
/// Built once, when this service's state is constructed, out of the fixed labels and
/// published chain facts in [`chain`]. `tip` is the instant it is built at, which is the
/// chain tip of every ledger it returns; no request ever adds to them.
fn seed_networks() -> BTreeMap<String, NetworkLedger> {
    let tip = now_secs();
    QUERY_NETWORK_VALUES
        .iter()
        .map(|network| ((*network).to_string(), chain::seed_ledger(network, tip)))
        .collect()
}

/// A deterministic 64-character lowercase hex digest of `seed`.
///
/// SHA-256 so each fixture identifier is a fresh-looking 32-byte value of exactly the
/// length the modelled hash patterns demand, and the same every run.
fn digest_hex(seed: &str) -> String {
    ::hex::encode(Sha256::digest(seed.as_bytes()))
}

/// A deterministic Ethereum-shaped address for `label` on `network`: `0x` plus 20
/// bytes, matching `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$`.
fn ethereum_address(network: &str, label: &str) -> String {
    format!("0x{}", &digest_hex(&format!("{network}/address/{label}"))[..40])
}

/// A deterministic Bitcoin-shaped address for `label` on `network`: a `1` prefix plus
/// 32 base58 characters, matching `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$`.
///
/// Base58 (and not hex or base64) because a Bitcoin address is base58: the alphabet
/// excludes `0`, `O`, `I` and `l`, so a fixture address cannot be misread.
fn bitcoin_address(network: &str, label: &str) -> String {
    const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let digest = Sha256::digest(format!("{network}/address/{label}").as_bytes());
    let mut address = String::from("1");
    for byte in digest.iter() {
        address.push(BASE58[*byte as usize % BASE58.len()] as char);
    }
    address
}

/// The value of the model's `QueryNetwork` enum (see
/// [`crate::ops::types::QUERY_NETWORK_VALUES`]) that names the Ethereum mainnet.
const ETHEREUM_MAINNET: &str = "ETHEREUM_MAINNET";

/// The well-known ERC-20 token contracts of the Ethereum mainnet, as
/// `(contractAddress, name, symbol, decimals)`.
///
/// `GetAssetContract`'s documentation: "Metadata is currently only available for some
/// ERC-20 contracts" — the metadata this API serves is an index of the chain's
/// well-known token contracts, so the emulator's mainnet ledger holds that same class of
/// contract rather than only synthetic ones. Each row is a public property of the
/// Ethereum mainnet: the address the contract is deployed at, and the `name`, `symbol`
/// and `decimals` its ERC-20 interface reports.
const ETHEREUM_MAINNET_ERC20: &[(&str, &str, &str, i32)] = &[
    (
        "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
        "USD Coin",
        "USDC",
        6,
    ),
    (
        "0xdac17f958d2ee523a2206206994597c13d831ec7",
        "Tether USD",
        "USDT",
        6,
    ),
    (
        "0x6b175474e89094c44da98b954eedeac495271d0f",
        "Dai Stablecoin",
        "DAI",
        18,
    ),
    (
        "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
        "Wrapped Ether",
        "WETH",
        18,
    ),
    (
        "0x2260fac5e5542a773aa44fbcfedf7c193bc2c599",
        "Wrapped BTC",
        "WBTC",
        8,
    ),
    (
        "0x514910771af9ca656af840dff83e8264ecf986ca",
        "ChainLink Token",
        "LINK",
        18,
    ),
];

/// [`ETHEREUM_MAINNET_ERC20`] as stored contracts.
///
/// `deployerAddress` ("the address of the deployer of contract") is a ledger-local
/// address: this emulator indexes no historical deployment transaction, so the deployer
/// is derived the way every other fixture address is, per contract and stable across
/// runs.
fn well_known_mainnet_erc20(network: &str) -> Vec<StoredContract> {
    ETHEREUM_MAINNET_ERC20
        .iter()
        .map(|(address, name, symbol, decimals)| StoredContract {
            network: network.to_string(),
            contract_address: (*address).to_string(),
            token_standard: "ERC20".to_string(),
            deployer_address: ethereum_address(network, &format!("deployer-{symbol}")),
            metadata: Some(ContractMetadata {
                name: Some((*name).to_string()),
                symbol: Some((*symbol).to_string()),
                decimals: Some(*decimals),
            }),
        })
        .collect()
}

/// The [`HoldingKey`] of a native Ether balance.
fn native_eth(owner: &str) -> HoldingKey {
    HoldingKey {
        contract_address: None,
        token_id: Some(NATIVE_TOKEN_ETH.to_string()),
        owner_address: owner.to_string(),
    }
}

/// The [`HoldingKey`] of a native Bitcoin balance.
fn native_btc(owner: &str) -> HoldingKey {
    HoldingKey {
        contract_address: None,
        token_id: Some(NATIVE_TOKEN_BTC.to_string()),
        owner_address: owner.to_string(),
    }
}

/// The [`HoldingKey`] of an ERC-20 balance: one fungible balance per contract, so no
/// `tokenId`.
fn erc20_holding(contract: &str, owner: &str) -> HoldingKey {
    HoldingKey {
        contract_address: Some(contract.to_string()),
        token_id: None,
        owner_address: owner.to_string(),
    }
}

/// Turn the fixture's transactions into a ledger, computing everything that depends on
/// the ledger as a whole: block identity and timestamps, each transaction's index
/// within its block and the number of transactions in it, cumulative gas, the fee, and
/// the holdings the events settle to.
fn build_ledger(
    network: &str,
    seeds: Vec<SeedTx>,
    opening: Vec<(HoldingKey, i128)>,
    contracts: Vec<StoredContract>,
    start: i64,
) -> NetworkLedger {
    let mut ledger = NetworkLedger {
        network: network.to_string(),
        ..NetworkLedger::default()
    };
    for contract in contracts {
        ledger.contracts.insert(
            normalize_contract_address(&contract.contract_address),
            contract,
        );
    }

    let bitcoin = is_bitcoin(network);
    // How many transactions each block holds — `Transaction$numberOfTransactions` is
    // "the number of transactions in the block", so it is counted, not declared.
    let mut per_block: BTreeMap<i64, i64> = BTreeMap::new();
    for seed in &seeds {
        *per_block.entry(seed.block).or_default() += 1;
    }

    // Which outputs this ledger's own inputs spend. `TransactionEvent$voutSpent`
    // "specifies if the transaction output is spent or unspent", so it is DERIVED from
    // the inputs that name an outpoint rather than declared on the output: an output no
    // transaction in this ledger spends is unspent, and one some transaction spends
    // cannot report otherwise.
    let spent_outputs: HashSet<(String, i32)> = seeds
        .iter()
        .flat_map(|seed| seed.events.iter())
        .filter_map(|event| {
            let hash = event.spent_vout_transaction_hash.as_deref()?;
            Some((normalize_hash(hash), event.spent_vout_index?))
        })
        .collect();

    let mut index_in_block: BTreeMap<i64, i64> = BTreeMap::new();
    let mut cumulative_gas: BTreeMap<i64, i128> = BTreeMap::new();

    for seed in seeds {
        let index = *index_in_block
            .entry(seed.block)
            .and_modify(|next| *next += 1)
            .or_insert(0);
        let cumulative = seed.gas_used.map(|gas| {
            let running = cumulative_gas.entry(seed.block).or_default();
            *running += gas;
            *running
        });
        // An Ethereum transaction's fee is the gas it burned at the price it paid; a
        // Bitcoin transaction's is what its inputs spent beyond what its outputs
        // created.
        let fee = if bitcoin {
            Some(event_total(&seed.events, "BITCOIN_VIN") - event_total(&seed.events, "BITCOIN_VOUT"))
        } else {
            seed.gas_used.map(|gas| gas * EFFECTIVE_GAS_PRICE)
        };
        let block_time = start + (seed.block + 1) * chain::FIXTURE_BLOCK_INTERVAL;
        let block_identifier = digest_hex(&format!("{network}/block/{}", seed.block));
        let block_hash = if bitcoin {
            block_identifier
        } else {
            format!("0x{block_identifier}")
        };
        let signature = |part: &str| format!("0x{}", digest_hex(&format!("{}/sig/{part}", seed.hash)));

        // An event that carries an output index IS an output, so it reports whether the
        // ledger holds an input spending it.
        let outpoint_prefix = normalize_hash(&seed.hash);
        let mut events = seed.events;
        for event in &mut events {
            if let Some(vout_index) = event.vout_index {
                event.vout_spent =
                    Some(spent_outputs.contains(&(outpoint_prefix.clone(), vout_index)));
            }
        }

        ledger.insert_transaction(StoredTransaction {
            network: network.to_string(),
            transaction_hash: seed.hash.clone(),
            transaction_id: seed.transaction_id.clone(),
            block_hash: Some(block_hash),
            // The height of the block holding this instant, at the network's own target
            // block time ([`chain::block_height`]).
            block_number: Some(chain::block_height(network, block_time).to_string()),
            transaction_timestamp: block_time,
            transaction_index: index,
            number_of_transactions: per_block.get(&seed.block).copied().unwrap_or(1),
            to: seed.to.clone(),
            from: seed.from.clone(),
            contract_address: seed.contract_address.clone(),
            gas_used: seed.gas_used,
            cumulative_gas_used: cumulative,
            effective_gas_price: seed.gas_used.map(|_| EFFECTIVE_GAS_PRICE),
            transaction_fee: fee,
            // The signature is an Ethereum transaction's; a Bitcoin transaction signs
            // each input instead, which the modelled `Transaction` has no member for.
            signature_v: (!bitcoin).then_some(27 + (index as i32 % 2)),
            signature_r: (!bitcoin).then(|| signature("r")),
            signature_s: (!bitcoin).then(|| signature("s")),
            confirmation_status: seed.confirmation_status.to_string(),
            execution_status: seed.execution_status.map(str::to_string),
            events,
        });
    }

    // Every balance opens at the instant the fixture's chain starts, which precedes
    // every transaction above.
    for (key, balance) in opening {
        ledger.record_opening(key, start, balance);
    }
    ledger.recompute_holdings();
    ledger
}

/// The total value the events of one transaction carry for `event_type`.
fn event_total(events: &[StoredEvent], event_type: &str) -> i128 {
    events
        .iter()
        .filter(|event| event.event_type == event_type)
        .filter_map(|event| event.value)
        .sum()
}

/// Derive every holding from the opening balances and the stored transactions.
///
/// Each opening balance carries the instant it was recorded at, which precedes every
/// transaction of its own holding; the transactions are then replayed in chain order
/// (the order the `BTreeMap`'s [`TxKey`] puts them in), so each holding's history comes
/// out ascending by time however the transactions entered the ledger.
///
/// This is why a balance can never disagree with the events that produced it: the
/// balance operations read holdings, and holdings exist only as the result of replaying
/// the events `ListTransactionEvents` returns, in chain order. A transaction whose
/// `executionStatus` is `FAILED` moved no value, so it is skipped.
///
/// Only the value an event carries moves a balance. On the Bitcoin networks the fee is
/// already part of that: the `BITCOIN_VIN` event spends the whole previous output and
/// the `BITCOIN_VOUT` event creates less, so the difference leaves the sender. On the
/// Ethereum networks the gas fee is reported on the transaction
/// (`Transaction$transactionFee`) but is not an event, so the native balances here
/// reconcile with the transfer events alone and are not fee-inclusive.
fn derive_holdings(
    transactions: &BTreeMap<TxKey, StoredTransaction>,
    opening: &[(HoldingKey, BalanceAt)],
) -> BTreeMap<HoldingKey, StoredHolding> {
    let mut holdings: BTreeMap<HoldingKey, StoredHolding> = BTreeMap::new();
    for (key, opened) in opening {
        // Keyed by the spelling a holding is looked up under, so an opening balance and
        // the events that move it land on ONE holding whichever way each spelled the
        // address.
        holdings
            .entry(key.normalized())
            .or_default()
            .record(opened.time, opened.balance);
    }

    for transaction in transactions.values() {
        if transaction.execution_status.as_deref() == Some(EXECUTION_FAILED) {
            continue;
        }
        for event in &transaction.events {
            let Some(value) = event.value else { continue };
            let Some((contract_address, token_id)) = event.holding() else {
                continue;
            };
            for (owner, delta) in [(&event.from, -value), (&event.to, value)] {
                let Some(owner) = owner else { continue };
                let key = HoldingKey {
                    contract_address: contract_address.clone(),
                    token_id: token_id.clone(),
                    owner_address: owner.clone(),
                }
                .normalized();
                let holding = holdings.entry(key).or_default();
                let current = holding.history.last().map_or(0, |record| record.balance);
                holding.record(transaction.transaction_timestamp, current + delta);
            }
        }
    }

    holdings
}
