//! $PICK — Parimutuel treasury memecoin (NEP-141 subset) + prediction events.
//!
//! Design notes (see README for the full trust model):
//! - Transfer tax on user->user transfers: part burns, part funds a treasury.
//! - The treasury funds parimutuel winner pools; a fixed slice of losing
//!   stakes burns at resolution (frozen params, no admin key).
//! - All money math is synchronous in this contract. The passport contract
//!   only mirrors pick/result records (fire-and-forget) and gates tier perks
//!   via `has_passport`, registered by the passport contract itself.
//! - Not implemented (documented in README): NEP-145 storage deposits,
//!   allowances, `ft_transfer_call`.
//!
//! (The lints below come from the third-party `uint` macro expansion.)
#![allow(deprecated, semicolon_in_expressions_from_non_local_macros)]

use std::collections::HashMap;

use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseResult,
};

use uint::construct_uint;

// The `uint` macro (third-party expansion; lints allowed at crate top).
construct_uint! { pub struct U256(4); }

// ---------------------------------------------------------------------------
// overflow-safe integer helpers
// ---------------------------------------------------------------------------

fn u256_from(v: u128) -> U256 {
    U256([v as u64, (v >> 64) as u64, 0, 0])
}

fn u128_from(v: U256) -> u128 {
    assert!(v.0[2] == 0 && v.0[3] == 0, "u128 overflow");
    v.0[0] as u128 | ((v.0[1] as u128) << 64)
}

/// a * b / c with no intermediate overflow (U256 backstop).
fn mul_div(a: u128, b: u128, c: u128) -> u128 {
    assert!(c > 0, "division by zero");
    u128_from(u256_from(a) * u256_from(b) / u256_from(c))
}

const BPS: u128 = 10_000;
const HISTORY_CAP: usize = 2_000;

fn now_sec() -> u64 {
    env::block_timestamp() / 1_000_000_000
}

// ---------------------------------------------------------------------------
// storage types
// ---------------------------------------------------------------------------

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Params {
    /// Creates and resolves events. Fixed at deploy, readable via get_params.
    pub resolver: AccountId,
    /// The passport contract allowed to call `register_passport`.
    pub passport: AccountId,
    pub transfer_tax_bps: u16,
    pub tax_burn_bps: u16,
    pub event_bonus_bps: u16,
    pub loser_burn_bps: u16,
    pub trader_bonus_bps: u16,
    pub oracle_bonus_bps: u16,
    pub trader_wins: u32,
    pub oracle_wins: u32,
    pub claim_window_secs: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct FtMetadata {
    pub spec: String,
    pub name: String,
    pub symbol: String,
    pub icon: Option<String>,
    pub decimals: u8,
    pub reference: Option<String>,
    pub reference_hash: Option<String>,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize, PartialEq)]
#[serde(crate = "near_sdk::serde")]
pub enum Status {
    Open,
    Resolved { winner: u8, resolve_ts: u64 },
    Void { void_ts: u64 },
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct Event {
    pub id: u64,
    pub question: String,
    pub side_a: String,
    pub side_b: String,
    pub close_ts: u64,
    pub resolve_deadline: u64,
    pub status: Status,
    pub stake_a: u128,
    pub stake_b: u128,
    /// Total owed to winners once resolved (= W + L - burn + bonus).
    pub winner_pool: u128,
    /// Total stakes on the winning side.
    pub winner_total: u128,
    pub paid_out: u128,
    pub swept: bool,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct Pick {
    pub side: u8,
    pub amount: u128,
    pub settled: bool,
    pub claimed: bool,
    pub refunded: bool,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Default)]
pub struct Stats {
    pub streak: u64,
    pub best: u64,
    pub wins: u32,
    pub losses: u32,
    pub picks: u32,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize, PartialEq)]
#[serde(crate = "near_sdk::serde", rename_all = "snake_case")]
pub enum LogKind {
    Stake,
    Tax,
    Resolve,
    Void,
    Claim,
    Refund,
    Sweep,
    Register,
    Unregister,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct LogEntry {
    pub ts: u64,
    pub kind: LogKind,
    pub account: AccountId,
    /// `u64::MAX` for entries not tied to an event.
    pub event_id: u64,
    pub amount: U128,
}

// ---------------------------------------------------------------------------
// view types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct EventView {
    pub id: u64,
    pub question: String,
    pub side_a: String,
    pub side_b: String,
    pub close_ts: u64,
    pub resolve_deadline: u64,
    pub status: Status,
    pub stake_a: U128,
    pub stake_b: U128,
    pub winner_pool: U128,
    pub winner_total: U128,
    pub paid_out: U128,
    pub swept: bool,
}

impl From<&Event> for EventView {
    fn from(e: &Event) -> Self {
        Self {
            id: e.id,
            question: e.question.clone(),
            side_a: e.side_a.clone(),
            side_b: e.side_b.clone(),
            close_ts: e.close_ts,
            resolve_deadline: e.resolve_deadline,
            status: e.status.clone(),
            stake_a: U128(e.stake_a),
            stake_b: U128(e.stake_b),
            winner_pool: U128(e.winner_pool),
            winner_total: U128(e.winner_total),
            paid_out: U128(e.paid_out),
            swept: e.swept,
        }
    }
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PickView {
    pub side: u8,
    pub amount: U128,
    pub settled: bool,
    pub claimed: bool,
    pub refunded: bool,
}

impl From<&Pick> for PickView {
    fn from(p: &Pick) -> Self {
        Self {
            side: p.side,
            amount: U128(p.amount),
            settled: p.settled,
            claimed: p.claimed,
            refunded: p.refunded,
        }
    }
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct StatsView {
    pub streak: u64,
    pub best: u64,
    pub wins: u32,
    pub losses: u32,
    pub picks: u32,
    pub has_passport: bool,
    pub tier: String,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct LeaderRow {
    pub account: AccountId,
    pub streak: u64,
    pub best: u64,
    pub wins: u32,
    pub losses: u32,
    pub tier: String,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct RecordPickArgs {
    user: AccountId,
    event_id: u64,
    side: u8,
    amount: U128,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct RecordResultArgs {
    user: AccountId,
    event_id: u64,
    outcome: u8,
    payout: U128,
}

// ---------------------------------------------------------------------------
// NEP-145 storage-management types
// ---------------------------------------------------------------------------

/// Minimum NEAR an account must deposit to register for transfers.
/// Covers the ledger entry the account will occupy.
pub const STORAGE_MIN: u128 = 5_000_000_000_000_000_000_000; // 0.005 NEAR

#[derive(Serialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct StorageBalance {
    pub total: U128,
    pub available: U128,
}

#[derive(Serialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct StorageBalanceBounds {
    pub min: U128,
    pub max: Option<U128>,
}

// ---------------------------------------------------------------------------
// contract
// ---------------------------------------------------------------------------

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Coin {
    pub params: Params,
    pub meta: FtMetadata,
    pub total_supply: u128,
    pub balances: HashMap<AccountId, u128>,
    /// Unresolved obligations: open stakes + funded but unclaimed pools.
    /// Invariant asserted in tests: balance[contract] >= escrow.
    pub escrow: u128,
    pub events: Vec<Event>,
    pub picks: HashMap<(AccountId, u64), Pick>,
    pub stats: HashMap<AccountId, Stats>,
    pub has_passport: HashMap<AccountId, bool>,
    pub users: Vec<AccountId>,
    pub history: Vec<LogEntry>,
    /// NEP-145: yoctoNEAR each registered account has deposited for storage.
    /// Invariant: env::account_balance() >= storage_total.
    pub storage: HashMap<AccountId, u128>,
    pub storage_total: u128,
}

#[near]
impl Coin {
    #[init]
    pub fn new(params: Params, meta: FtMetadata, initial_supply: U128) -> Self {
        assert!(params.transfer_tax_bps as u128 <= BPS, "tax > 100%");
        assert!(params.tax_burn_bps as u128 <= BPS, "burn bps > 100%");
        assert!(params.event_bonus_bps as u128 <= BPS, "bonus bps > 100%");
        assert!(params.loser_burn_bps as u128 <= BPS, "loser burn > 100%");
        assert!(params.trader_wins > 0, "trader_wins = 0");
        assert!(
            params.oracle_wins >= params.trader_wins,
            "oracle_wins < trader_wins"
        );
        assert!(params.claim_window_secs > 0, "claim window = 0");
        assert!(initial_supply.0 > 0, "empty supply");
        assert!(!meta.symbol.is_empty(), "empty symbol");
        assert!(params.resolver != params.passport, "resolver = passport");
        let owner = env::predecessor_account_id();
        let mut balances = HashMap::new();
        balances.insert(owner, initial_supply.0);
        Self {
            params,
            meta,
            total_supply: initial_supply.0,
            balances,
            escrow: 0,
            events: Vec::new(),
            picks: HashMap::new(),
            stats: HashMap::new(),
            has_passport: HashMap::new(),
            users: Vec::new(),
            history: Vec::new(),
            storage: HashMap::new(),
            storage_total: 0,
        }
    }

    // -----------------------------------------------------------------
    // NEP-141 fungible token
    // -----------------------------------------------------------------

    /// Transfers require both accounts registered via NEP-145 (`storage_deposit`).
    pub fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        assert_eq!(
            env::attached_deposit(),
            NearToken::from_yoctonear(0),
            "no deposit expected"
        );
        assert!(amount.0 > 0, "zero amount");
        let sender = env::predecessor_account_id();
        self.assert_registered(&sender);
        self.assert_registered(&receiver_id);
        self.internal_transfer(&sender, &receiver_id, amount.0, true);
        if let Some(m) = memo {
            env::log_str(&format!("Memo: {} ({} -> {})", m, sender, receiver_id));
        }
    }

    /// NEP-141 transfer-and-call: moves tokens (with the usual tax), then calls
    /// `ft_on_transfer` on the receiver and settles the unused portion back
    /// through the `ft_resolve_transfer` callback. Refunds are internal
    /// (untaxed) and capped by what the receiver actually received — so the
    /// transfer tax is charged once and is not undone by a refund.
    pub fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> Promise {
        assert_eq!(
            env::attached_deposit(),
            NearToken::from_yoctonear(0),
            "no deposit expected"
        );
        assert!(amount.0 > 0, "zero amount");
        let sender = env::predecessor_account_id();
        self.assert_registered(&sender);
        self.assert_registered(&receiver_id);
        let tax = self.internal_transfer(&sender, &receiver_id, amount.0, true);
        let _received = amount.0 - tax;
        if let Some(m) = memo {
            env::log_str(&format!("Memo: {} ({} -> {})", m, sender, receiver_id));
        }

        let on_transfer_args = near_sdk::serde_json::json!({
            "sender_id": sender,
            "amount": U128(amount.0),
            "msg": msg,
        })
        .to_string()
        .into_bytes();
        let resolve_args = near_sdk::serde_json::json!({
            "sender_id": sender,
            "receiver_id": receiver_id,
            "amount": U128(amount.0),
        })
        .to_string()
        .into_bytes();

        // ft_on_transfer may fail (missing method, panic) — the callback still
        // runs and treats that as "everything unused", refunding in full.
        Promise::new(receiver_id).function_call(
            "ft_on_transfer".to_string(),
            on_transfer_args,
            NearToken::from_yoctonear(0),
            Gas::from_tgas(35),
        )
        .then(Promise::new(env::current_account_id()).function_call(
            "ft_resolve_transfer".to_string(),
            resolve_args,
            NearToken::from_yoctonear(0),
            Gas::from_tgas(20),
        ))
    }

    /// Callback for `ft_transfer_call`. Returns the amount refunded to the
    /// sender. Only reachable as a promise continuation: a direct call has no
    /// promise result (and a non-contract predecessor fails the first check).
    pub fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
    ) -> U128 {
        assert_eq!(
            env::predecessor_account_id(),
            env::current_account_id(),
            "callback only"
        );
        assert_eq!(env::promise_results_count(), 1, "missing ft_on_transfer result");

        let contract_id = env::current_account_id();
        let is_operational = sender_id == contract_id || receiver_id == contract_id;
        let received = if is_operational {
            amount.0
        } else {
            amount.0 - mul_div(amount.0, self.params.transfer_tax_bps as u128, BPS)
        };

        // Successful result -> unused amount as declared by the receiver
        // (accepts both JSON encodings: "4000" and 4000). Failed result ->
        // refund everything the receiver got.
        let unused: u128 = match env::promise_result(0) {
            PromiseResult::Successful(bytes) => {
                near_sdk::serde_json::from_slice::<U128>(&bytes)
                    .map(|u| u.0)
                    .or_else(|_| near_sdk::serde_json::from_slice::<u128>(&bytes))
                    .unwrap_or(received)
            }
            PromiseResult::Failed => received,
        };

        let refund = unused
            .min(received)
            .min(self.balances.get(&receiver_id).copied().unwrap_or(0));
        if refund == 0 {
            return U128(0);
        }
        self.internal_transfer(&receiver_id, &sender_id, refund, false);
        U128(refund)
    }

    pub fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        U128(self.balances.get(&account_id).copied().unwrap_or(0))
    }

    pub fn ft_total_supply(&self) -> U128 {
        U128(self.total_supply)
    }

    pub fn ft_metadata(&self) -> FtMetadata {
        self.meta.clone()
    }

    // -----------------------------------------------------------------
    // NEP-145 storage management
    // -----------------------------------------------------------------

    pub fn storage_balance_bounds(&self) -> StorageBalanceBounds {
        StorageBalanceBounds {
            min: U128(STORAGE_MIN),
            max: None,
        }
    }

    pub fn storage_balance_of(&self, account_id: AccountId) -> Option<StorageBalance> {
        self.storage.get(&account_id).map(|&total| StorageBalance {
            total: U128(total),
            available: U128(total.saturating_sub(STORAGE_MIN)),
        })
    }

    /// Register `account_id` (default: caller) by depositing NEAR for the
    /// ledger entry. Deposits below `min` panic. Re-depositing tops up
    /// `available`; `registration_only` on an existing account refunds.
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        registration_only: Option<bool>,
    ) -> StorageBalance {
        let attached = env::attached_deposit().as_yoctonear();
        let target = account_id.unwrap_or_else(env::predecessor_account_id);
        let registration_only = registration_only.unwrap_or(false);

        if let Some(&existing) = self.storage.get(&target) {
            if registration_only {
                // already registered: nothing to do, return the deposit
                if attached > 0 {
                    let _ = Promise::new(env::predecessor_account_id())
                        .transfer(NearToken::from_yoctonear(attached));
                }
                return StorageBalance {
                    total: U128(existing),
                    available: U128(existing.saturating_sub(STORAGE_MIN)),
                };
            }
            assert!(attached > 0, "top-up deposit is zero");
            *self.storage.get_mut(&target).expect("storage") += attached;
            self.storage_total += attached;
        } else {
            assert!(
                attached >= STORAGE_MIN,
                "attached deposit is below min storage balance"
            );
            self.storage.insert(target.clone(), attached);
            self.storage_total += attached;
        }
        self.assert_near_solvency();
        let total = self.storage.get(&target).copied().unwrap_or(0);
        StorageBalance {
            total: U128(total),
            available: U128(total.saturating_sub(STORAGE_MIN)),
        }
    }

    /// Returns up to `amount` (default: all) of the withdrawable deposit.
    pub fn storage_withdraw(&mut self, amount: Option<U128>) -> StorageBalance {
        assert_eq!(
            env::attached_deposit(),
            NearToken::from_yoctonear(0),
            "no deposit expected"
        );
        let user = env::predecessor_account_id();
        let total = *self.storage.get(&user).expect("not registered");
        let available = total.saturating_sub(STORAGE_MIN);
        let want = amount.map(|a| a.0).unwrap_or(available);
        assert!(want <= available, "amount exceeds available storage balance");
        if want > 0 {
            self.storage.insert(user.clone(), total - want);
            self.storage_total -= want;
            let _ = Promise::new(user).transfer(NearToken::from_yoctonear(want));
        }
        let new_total = total - want;
        StorageBalance {
            total: U128(new_total),
            available: U128(new_total.saturating_sub(STORAGE_MIN)),
        }
    }

    /// Unregister and get the NEAR deposit back. Refuses while a balance
    /// exists unless `force`; force-burning destroys the account's $PICK
    /// (reducing total supply) and logs it.
    pub fn storage_unregister(&mut self, force: Option<bool>) -> bool {
        assert_eq!(
            env::attached_deposit(),
            NearToken::from_yoctonear(0),
            "no deposit expected"
        );
        let force = force.unwrap_or(false);
        let user = env::predecessor_account_id();
        let Some(&deposit) = self.storage.get(&user) else {
            return false;
        };
        let balance = self.balances.get(&user).copied().unwrap_or(0);
        if balance > 0 {
            assert!(force, "account has a $PICK balance; burn it with force");
            self.total_supply -= balance;
            self.balances.remove(&user);
            self.push_log(LogKind::Unregister, user.clone(), u64::MAX, U128(balance));
        } else {
            self.balances.remove(&user);
        }
        self.storage.remove(&user);
        self.storage_total -= deposit;
        if deposit > 0 {
            let _ = Promise::new(user).transfer(NearToken::from_yoctonear(deposit));
        }
        true
    }

    fn assert_registered(&self, account: &AccountId) {
        assert!(
            self.storage.contains_key(account),
            "account not registered — call storage_deposit first (NEP-145)"
        );
    }

    fn assert_near_solvency(&self) {
        assert!(
            env::account_balance().as_yoctonear() >= self.storage_total,
            "contract NEAR below storage obligations"
        );
    }

    /// Returns the tax taken (0 when exempt).
    fn internal_transfer(&mut self, from: &AccountId, to: &AccountId, amount: u128, taxed: bool) -> u128 {
        let contract_id = env::current_account_id();
        let is_operational = from == &contract_id || to == &contract_id;
        let (tax, burn) = if taxed && !is_operational {
            let tax = mul_div(amount, self.params.transfer_tax_bps as u128, BPS);
            let burn = mul_div(tax, self.params.tax_burn_bps as u128, BPS);
            (tax, burn)
        } else {
            (0, 0)
        };
        let fb = self.balances.entry(from.clone()).or_insert(0);
        assert!(*fb >= amount, "insufficient balance");
        *fb -= amount;
        *self.balances.entry(to.clone()).or_insert(0) += amount - tax;
        *self.balances.entry(contract_id).or_insert(0) += tax - burn;
        self.total_supply -= burn;
        if burn > 0 {
            self.push_log(LogKind::Tax, from.clone(), u64::MAX, U128(burn));
        }
        tax
    }

    // -----------------------------------------------------------------
    // events: create / stake / resolve / claim / refund
    // -----------------------------------------------------------------

    pub fn create_event(
        &mut self,
        question: String,
        side_a: String,
        side_b: String,
        close_ts: u64,
        resolve_deadline: u64,
    ) -> u64 {
        assert_eq!(
            env::predecessor_account_id(),
            self.params.resolver,
            "only resolver"
        );
        assert!(!question.is_empty() && question.len() <= 280, "question length");
        assert!(
            !side_a.is_empty() && side_a.len() <= 64 && !side_b.is_empty() && side_b.len() <= 64,
            "side label length"
        );
        let now = now_sec();
        assert!(close_ts > now, "close must be in the future");
        assert!(resolve_deadline > close_ts, "deadline must be after close");
        let id = self.events.len() as u64;
        self.events.push(Event {
            id,
            question,
            side_a,
            side_b,
            close_ts,
            resolve_deadline,
            status: Status::Open,
            stake_a: 0,
            stake_b: 0,
            winner_pool: 0,
            winner_total: 0,
            paid_out: 0,
            swept: false,
        });
        id
    }

    /// Stake $PICK on one side. One pick per user per event (top-ups allowed
    /// on the same side; the opposite side panics).
    pub fn stake(&mut self, event_id: u64, side: u8, amount: U128) {
        let amount = amount.0;
        assert!(amount > 0, "zero stake");
        assert!(side < 2, "side must be 0 or 1");
        let now = now_sec();
        let user = env::predecessor_account_id();
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");

        {
            let ev = &mut self.events[idx];
            assert!(matches!(ev.status, Status::Open), "event not open");
            assert!(now <= ev.close_ts, "betting closed");
            if side == 0 {
                ev.stake_a += amount;
            } else {
                ev.stake_b += amount;
            }
        }

        let bal = self.balances.entry(user.clone()).or_insert(0);
        assert!(*bal >= amount, "insufficient balance");
        *bal -= amount;
        // escrowed stakes live in the contract's own balance
        *self
            .balances
            .entry(env::current_account_id())
            .or_insert(0) += amount;
        self.escrow += amount;

        if !self.stats.contains_key(&user) {
            self.stats.insert(user.clone(), Stats::default());
            self.users.push(user.clone());
        }

        let key = (user.clone(), event_id);
        match self.picks.get_mut(&key) {
            Some(p) => {
                assert_eq!(p.side, side, "already staked the other side");
                p.amount += amount;
            }
            None => {
                self.picks.insert(
                    key,
                    Pick {
                        side,
                        amount,
                        settled: false,
                        claimed: false,
                        refunded: false,
                    },
                );
            }
        }

        self.push_log(LogKind::Stake, user.clone(), event_id, U128(amount));
        self.record_pick(user, event_id, side, amount);
    }

    /// Resolver reports the winning side. If either side has zero stakes the
    /// event becomes Void and everyone refunds.
    pub fn resolve(&mut self, event_id: u64, winner: u8) {
        assert_eq!(
            env::predecessor_account_id(),
            self.params.resolver,
            "only resolver"
        );
        assert!(winner < 2, "side must be 0 or 1");
        let now = now_sec();
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");

        let mut is_void = false;
        {
            let ev = &mut self.events[idx];
            assert!(matches!(ev.status, Status::Open), "event not open");
            assert!(now <= ev.resolve_deadline, "resolution window passed");

            if ev.stake_a == 0 || ev.stake_b == 0 {
                ev.status = Status::Void { void_ts: now };
                is_void = true;
            } else {
                let win_total = if winner == 0 { ev.stake_a } else { ev.stake_b };
                let lose_total = if winner == 0 { ev.stake_b } else { ev.stake_a };
                let lose_burn = mul_div(lose_total, self.params.loser_burn_bps as u128, BPS);
                let contract = env::current_account_id();
                let balance = self.balances.get(&contract).copied().unwrap_or(0);
                let free = balance - self.escrow; // invariant: balance >= escrow
                let bonus = mul_div(free, self.params.event_bonus_bps as u128, BPS);

                if lose_burn > 0 {
                    *self.balances.get_mut(&contract).expect("contract") -= lose_burn;
                    self.total_supply -= lose_burn;
                }
                self.escrow = self.escrow - lose_burn + bonus;
                ev.winner_pool = win_total + lose_total - lose_burn + bonus;
                ev.winner_total = win_total;
                ev.status = Status::Resolved { winner, resolve_ts: now };
            }
        }

        if is_void {
            self.push_log(LogKind::Void, env::predecessor_account_id(), event_id, U128(0));
        } else {
            let pool = self.events[idx].winner_pool;
            self.push_log(
                LogKind::Resolve,
                env::predecessor_account_id(),
                event_id,
                U128(pool),
            );
        }
    }

    /// Permissionless safety valve: void an event after close if one side
    /// never received a single stake.
    pub fn mark_void(&mut self, event_id: u64) {
        let now = now_sec();
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");
        let ev = &mut self.events[idx];
        assert!(matches!(ev.status, Status::Open), "event not open");
        assert!(now > ev.close_ts, "betting still open");
        assert!(ev.stake_a == 0 || ev.stake_b == 0, "both sides have stakes");
        ev.status = Status::Void { void_ts: now };
        self.push_log(LogKind::Void, env::predecessor_account_id(), event_id, U128(0));
    }

    /// Record win/loss/streak for the caller's pick (no money movement).
    pub fn settle(&mut self, event_id: u64) {
        let user = env::predecessor_account_id();
        self.settle_pick(&user, event_id);
    }

    /// Permissionless version: anyone can settle anyone's resolved pick, so
    /// records can't be hidden by simply not claiming.
    pub fn settle_for(&mut self, user: AccountId, event_id: u64) {
        self.settle_pick(&user, event_id);
    }

    fn settle_pick(&mut self, user: &AccountId, event_id: u64) {
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");
        let status = self.events[idx].status.clone();
        let key = (user.clone(), event_id);
        let pick = match self.picks.get_mut(&key) {
            Some(p) => p,
            None => panic!("no pick"),
        };
        if pick.settled {
            return;
        }
        match status {
            Status::Resolved { winner, .. } => {
                let correct = pick.side == winner;
                pick.settled = true;
                let st = self.stats.entry(user.clone()).or_default();
                st.picks += 1;
                if correct {
                    st.wins += 1;
                    st.streak += 1;
                    if st.streak > st.best {
                        st.best = st.streak;
                    }
                    // mirror the win immediately — the passport record must not
                    // depend on the winner ever calling claim() (the payout is
                    // filled in later by claim's own record_result)
                    self.record_result(user.clone(), event_id, 1, 0);
                } else {
                    st.losses += 1;
                    st.streak = 0;
                    self.record_result(user.clone(), event_id, 0, 0);
                }
            }
            Status::Void { .. } => {
                pick.settled = true;
                self.record_result(user.clone(), event_id, 2, 0);
            }
            Status::Open => panic!("event not resolved"),
        }
    }

    /// Winners claim stake + pool share + (with passport) tier bonus.
    pub fn claim(&mut self, event_id: u64) -> U128 {
        let user = env::predecessor_account_id();
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");
        let status = self.events[idx].status.clone();
        let resolve_ts = match &status {
            Status::Resolved { resolve_ts, .. } => *resolve_ts,
            Status::Void { .. } => panic!("event void — use refund()"),
            Status::Open => panic!("not resolved"),
        };
        {
            let ev = &self.events[idx];
            assert!(!ev.swept, "pool swept");
            assert!(
                now_sec() <= resolve_ts + self.params.claim_window_secs,
                "claim window passed"
            );
            assert!(ev.winner_total > 0, "no winners");
        }

        let key = (user.clone(), event_id);
        let (stake_amount, side) = {
            let p = self.picks.get(&key).expect("no pick");
            assert!(!p.claimed, "already claimed");
            assert!(!p.refunded, "already refunded");
            (p.amount, p.side)
        };
        let winner = match &status {
            Status::Resolved { winner, .. } => *winner,
            _ => unreachable!(),
        };
        assert_eq!(side, winner, "you lost — nothing to claim");

        // stats first (no-op if already settled) so the tier is up to date
        self.settle_pick(&user, event_id);

        let pool = self.events[idx].winner_pool;
        let winner_total = self.events[idx].winner_total;
        let payout = mul_div(stake_amount, pool, winner_total);

        let tier_bps = self.tier_bonus_bps(&user);
        let contract = env::current_account_id();
        let free = self.balances.get(&contract).copied().unwrap_or(0) - self.escrow;
        let bonus = if tier_bps > 0 {
            mul_div(payout, tier_bps as u128, BPS).min(free)
        } else {
            0
        };
        let total = payout + bonus;

        self.picks.get_mut(&key).expect("pick").claimed = true;
        self.escrow -= payout;
        self.events[idx].paid_out += payout;
        *self.balances.get_mut(&contract).expect("contract") -= total;
        *self.balances.entry(user.clone()).or_insert(0) += total;

        self.push_log(LogKind::Claim, user.clone(), event_id, U128(total));
        self.record_result(user.clone(), event_id, 1, payout);
        U128(total)
    }

    /// Get your stake back: Void events, or events the resolver never
    /// resolved before `resolve_deadline`.
    pub fn refund(&mut self, event_id: u64) -> U128 {
        let user = env::predecessor_account_id();
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");
        let status = self.events[idx].status.clone();
        let now = now_sec();
        match &status {
            Status::Void { .. } => {}
            Status::Open => {
                assert!(now > self.events[idx].resolve_deadline, "still open");
            }
            Status::Resolved { .. } => panic!("resolved — use claim()"),
        }

        let key = (user.clone(), event_id);
        let amount = {
            let p = self.picks.get_mut(&key).expect("no pick");
            assert!(!p.refunded, "already refunded");
            assert!(!p.claimed, "already claimed");
            let amount = p.amount;
            p.refunded = true;
            amount
        };
        self.escrow -= amount;
        *self.balances
            .get_mut(&env::current_account_id())
            .expect("contract") -= amount;
        *self.balances.entry(user.clone()).or_insert(0) += amount;

        self.push_log(LogKind::Refund, user, event_id, U128(amount));
        U128(amount)
    }

    /// After the claim window, unclaimed remainder returns to the free
    /// treasury (permissionless).
    pub fn sweep(&mut self, event_id: u64) {
        let idx = event_id as usize;
        assert!(idx < self.events.len(), "no event");
        let resolve_ts = {
            let ev = &self.events[idx];
            match &ev.status {
                Status::Resolved { resolve_ts, .. } => *resolve_ts,
                _ => panic!("not resolved"),
            }
        };
        let remaining = {
            let ev = &mut self.events[idx];
            assert!(!ev.swept, "already swept");
            assert!(
                now_sec() > resolve_ts + self.params.claim_window_secs,
                "claim window not passed"
            );
            let remaining = ev.winner_pool - ev.paid_out;
            ev.swept = true;
            remaining
        };
        self.escrow -= remaining;
        self.push_log(
            LogKind::Sweep,
            env::predecessor_account_id(),
            event_id,
            U128(remaining),
        );
    }

    // -----------------------------------------------------------------
    // passport integration
    // -----------------------------------------------------------------

    pub fn register_passport(&mut self, user: AccountId) {
        assert_eq!(
            env::predecessor_account_id(),
            self.params.passport,
            "only passport contract"
        );
        if !self.has_passport.get(&user).copied().unwrap_or(false) {
            self.has_passport.insert(user.clone(), true);
            if !self.stats.contains_key(&user) {
                self.stats.insert(user.clone(), Stats::default());
                self.users.push(user.clone());
            }
            self.push_log(LogKind::Register, user, u64::MAX, U128(0));
        }
    }

    fn tier_bonus_bps(&self, user: &AccountId) -> u16 {
        if !self.has_passport.get(user).copied().unwrap_or(false) {
            return 0;
        }
        let wins = self.stats.get(user).map(|s| s.wins).unwrap_or(0);
        if wins >= self.params.oracle_wins {
            self.params.oracle_bonus_bps
        } else if wins >= self.params.trader_wins {
            self.params.trader_bonus_bps
        } else {
            0
        }
    }

    fn tier_name(&self, user: &AccountId) -> String {
        if !self.has_passport.get(user).copied().unwrap_or(false) {
            return "-".to_string();
        }
        let wins = self.stats.get(user).map(|s| s.wins).unwrap_or(0);
        if wins >= self.params.oracle_wins {
            "ORACLE".to_string()
        } else if wins >= self.params.trader_wins {
            "TRADER".to_string()
        } else {
            "ROOKIE".to_string()
        }
    }

    fn record_pick(&self, user: AccountId, event_id: u64, side: u8, amount: u128) {
        let args = near_sdk::serde_json::to_vec(&RecordPickArgs {
            user,
            event_id,
            side,
            amount: U128(amount),
        })
        .expect("serialize");
        let _ = env::promise_create(
            self.params.passport.clone(),
            "record_pick",
            &args,
            NearToken::from_yoctonear(0),
            Gas::from_tgas(15),
        );
    }

    fn record_result(&self, user: AccountId, event_id: u64, outcome: u8, payout: u128) {
        let args = near_sdk::serde_json::to_vec(&RecordResultArgs {
            user,
            event_id,
            outcome,
            payout: U128(payout),
        })
        .expect("serialize");
        let _ = env::promise_create(
            self.params.passport.clone(),
            "record_result",
            &args,
            NearToken::from_yoctonear(0),
            Gas::from_tgas(15),
        );
    }

    // -----------------------------------------------------------------
    // logs
    // -----------------------------------------------------------------

    fn push_log(&mut self, kind: LogKind, account: AccountId, event_id: u64, amount: U128) {
        if self.history.len() >= HISTORY_CAP {
            self.history.drain(0..500);
        }
        self.history.push(LogEntry {
            ts: now_sec(),
            kind,
            account,
            event_id,
            amount,
        });
    }

    // -----------------------------------------------------------------
    // views
    // -----------------------------------------------------------------

    pub fn get_params(&self) -> Params {
        self.params.clone()
    }

    pub fn get_events(&self) -> Vec<EventView> {
        self.events.iter().map(EventView::from).collect()
    }

    pub fn get_event(&self, event_id: u64) -> Option<EventView> {
        self.events.get(event_id as usize).map(EventView::from)
    }

    pub fn get_pick(&self, user: AccountId, event_id: u64) -> Option<PickView> {
        self.picks.get(&(user, event_id)).map(PickView::from)
    }

    pub fn get_stats(&self, user: AccountId) -> Option<StatsView> {
        self.stats.get(&user).map(|st| StatsView {
            streak: st.streak,
            best: st.best,
            wins: st.wins,
            losses: st.losses,
            picks: st.picks,
            has_passport: self.has_passport.get(&user).copied().unwrap_or(false),
            tier: self.tier_name(&user),
        })
    }

    pub fn get_leaderboard(&self, limit: u32) -> Vec<LeaderRow> {
        let mut rows: Vec<LeaderRow> = self
            .stats
            .iter()
            .filter(|(_, st)| st.picks > 0)
            .map(|(acc, st)| LeaderRow {
                account: acc.clone(),
                streak: st.streak,
                best: st.best,
                wins: st.wins,
                losses: st.losses,
                tier: self.tier_name(acc),
            })
            .collect();
        rows.sort_by(|a, b| b.streak.cmp(&a.streak).then(b.wins.cmp(&a.wins)));
        rows.truncate(limit as usize);
        rows
    }

    pub fn get_history(&self, limit: u32) -> Vec<LogEntry> {
        let n = limit as usize;
        let len = self.history.len();
        if n >= len {
            return self.history.clone();
        }
        self.history[len - n..].to_vec()
    }

    pub fn free_treasury(&self) -> U128 {
        let contract = env::current_account_id();
        let balance = self.balances.get(&contract).copied().unwrap_or(0);
        U128(balance.saturating_sub(self.escrow))
    }

    pub fn has_passport_of(&self, user: AccountId) -> bool {
        self.has_passport.get(&user).copied().unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{accounts, testing_env_with_promise_results, VMContextBuilder};
    use near_sdk::testing_env;

    const DAY: u64 = 86_400;
    const S: u128 = 1_000_000_000;

    fn alice() -> AccountId {
        accounts(0)
    }
    fn bob() -> AccountId {
        accounts(1)
    }
    fn carol() -> AccountId {
        accounts(2)
    }
    fn contract() -> AccountId {
        accounts(3)
    }
    fn resolver() -> AccountId {
        accounts(4)
    }
    fn passport() -> AccountId {
        accounts(5)
    }

    fn set(pred: AccountId, now: u64) {
        let mut b = VMContextBuilder::new();
        b.current_account_id(contract())
            .predecessor_account_id(pred)
            .block_timestamp(now * 1_000_000_000);
        testing_env!(b.build());
    }

    fn set_dep(pred: AccountId, now: u64, deposit: u128) {
        let mut b = VMContextBuilder::new();
        b.current_account_id(contract())
            .predecessor_account_id(pred)
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .block_timestamp(now * 1_000_000_000);
        testing_env!(b.build());
    }

    /// NEP-145 registration (required before ft_transfer).
    fn register(c: &mut Coin, user: AccountId, now: u64) {
        set_dep(user.clone(), now, STORAGE_MIN);
        c.storage_deposit(None, None);
        set(user, now);
    }

    fn meta() -> FtMetadata {
        FtMetadata {
            spec: "ft-1.0.0".into(),
            name: "PICK".into(),
            symbol: "PICK".into(),
            icon: None,
            decimals: 9,
            reference: None,
            reference_hash: None,
        }
    }

    fn default_params() -> Params {
        Params {
            resolver: resolver(),
            passport: passport(),
            transfer_tax_bps: 100,
            tax_burn_bps: 5_000,
            event_bonus_bps: 200,
            loser_burn_bps: 2_000,
            trader_bonus_bps: 100,
            oracle_bonus_bps: 300,
            trader_wins: 5,
            oracle_wins: 10,
            claim_window_secs: 90 * DAY,
        }
    }

    fn init_with(p: Params) -> Coin {
        set(alice(), 1_000);
        Coin::new(p, meta(), U128(S))
    }

    fn init() -> Coin {
        init_with(default_params())
    }

    fn assert_invariants(c: &Coin) {
        let sum: u128 = c.balances.values().sum();
        assert_eq!(sum, c.total_supply, "balances != total supply");
        let bal = c.balances.get(&contract()).copied().unwrap_or(0);
        assert!(bal >= c.escrow, "escrow underwater: {} < {}", bal, c.escrow);
    }

    #[test]
    fn transfer_tax_burns_and_fills_treasury() {
        let mut c = init();
        register(&mut c, alice(), 1_500);
        register(&mut c, bob(), 1_500);
        set(alice(), 2_000);
        c.ft_transfer(bob(), U128(10_000), None);
        // 1% tax = 100, half burned (50), half to treasury (50)
        assert_eq!(c.ft_balance_of(alice()).0, S - 10_000);
        assert_eq!(c.ft_balance_of(bob()).0, 9_900);
        assert_eq!(c.ft_balance_of(contract()).0, 50);
        assert_eq!(c.ft_total_supply().0, S - 50);
        assert_eq!(c.free_treasury().0, 50);
        assert_invariants(&c);
    }

    #[test]
    fn stake_resolve_claim_flow() {
        let mut c = init();
        let now = 1_000u64;

        // fund bob: tax 500 (500 burned into treasury)
        register(&mut c, alice(), now);
        register(&mut c, bob(), now);
        set(alice(), now);
        c.ft_transfer(bob(), U128(50_000), None);

        set(resolver(), now);
        let id = c.create_event(
            "ETH above $5k on 14 Nov?".into(),
            "YES".into(),
            "NO".into(),
            now + DAY,
            now + 2 * DAY,
        );

        set(alice(), now + 100);
        c.stake(id, 0, U128(1_000));
        set(bob(), now + 100);
        c.stake(id, 1, U128(300));
        assert_invariants(&c);

        set(resolver(), now + 200);
        c.resolve(id, 0);
        // loser burn = 300 * 20% = 60; bonus = 250 free * 2% = 5
        // winner pool = 1000 + 300 - 60 + 5 = 1245, winner_total = 1000
        let ev = c.get_event(id).unwrap();
        assert_eq!(ev.winner_pool.0, 1_245);
        assert_eq!(ev.winner_total.0, 1_000);
        assert_invariants(&c);

        set(alice(), now + 300);
        let paid = c.claim(id).0;
        assert_eq!(paid, 1_245); // no passport -> no tier bonus
        let st = c.get_stats(alice()).unwrap();
        assert_eq!((st.wins, st.losses, st.streak), (1, 0, 1));
        assert_invariants(&c);

        // loser's stats recorded permissionlessly by a third party
        set(carol(), now + 400);
        c.settle_for(bob(), id);
        let bs = c.get_stats(bob()).unwrap();
        assert_eq!((bs.wins, bs.losses, bs.streak), (0, 1, 0));

        // double claim must panic
        set(alice(), now + 500);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.claim(id)));
        assert!(res.is_err());
        assert_invariants(&c);
    }

    #[test]
    fn single_sided_event_voids_and_refunds() {
        let mut c = init();
        let now = 1_000u64;
        set(resolver(), now);
        let id = c.create_event("q".into(), "A".into(), "B".into(), now + DAY, now + 2 * DAY);
        set(alice(), now + 10);
        c.stake(id, 0, U128(700));

        set(resolver(), now + DAY + 10);
        c.resolve(id, 1); // one side empty -> Void, not a loss
        assert!(matches!(c.get_event(id).unwrap().status, Status::Void { .. }));

        set(alice(), now + DAY + 20);
        assert_eq!(c.refund(id).0, 700);
        assert_eq!(c.ft_balance_of(alice()).0, S);
        assert_invariants(&c);
    }

    #[test]
    fn unresolved_after_deadline_refunds_and_resolver_is_locked_out() {
        let mut c = init();
        let now = 1_000u64;
        set(resolver(), now);
        let id = c.create_event("q".into(), "A".into(), "B".into(), now + DAY, now + 2 * DAY);
        register(&mut c, alice(), now);
        register(&mut c, bob(), now);
        set(alice(), now);
        c.ft_transfer(bob(), U128(10_000), None);
        set(alice(), now + 10);
        c.stake(id, 0, U128(500));
        set(bob(), now + 10);
        c.stake(id, 1, U128(200));

        set(resolver(), now + 2 * DAY + 1);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.resolve(id, 0)));
        assert!(res.is_err(), "resolve must fail after the deadline");

        set(alice(), now + 2 * DAY + 2);
        assert_eq!(c.refund(id).0, 500);
        assert_invariants(&c);
    }

    #[test]
    fn passport_tier_bonus_paid_on_claim() {
        let mut p = default_params();
        p.trader_wins = 1;
        p.oracle_wins = 2;
        let mut c = init_with(p);
        let now = 1_000u64;

        set(passport(), now);
        c.register_passport(alice());
        assert!(c.has_passport_of(alice()));

        // build treasury: alice funds bob (tax 500, burn 250) and carol
        // (tax 1_000, burn 500) -> free treasury 750
        register(&mut c, alice(), now);
        register(&mut c, bob(), now);
        register(&mut c, carol(), now);
        set(alice(), now);
        c.ft_transfer(bob(), U128(50_000), None);
        c.ft_transfer(carol(), U128(100_000), None);

        set(resolver(), now);
        let id = c.create_event("q".into(), "A".into(), "B".into(), now + DAY, now + 2 * DAY);
        set(alice(), now + 10);
        c.stake(id, 0, U128(1_000));
        set(bob(), now + 10);
        c.stake(id, 1, U128(1_000));
        set(resolver(), now + 20);
        c.resolve(id, 0);
        // loser burn 200, bonus 750 * 2% = 15 -> pool 1815 / winner_total 1000
        set(alice(), now + 30);
        let total = c.claim(id).0;
        // base payout 1815 + TRADER bonus 1% (18) = 1833
        assert_eq!(total, 1_833);
        let st = c.get_stats(alice()).unwrap();
        assert_eq!(st.tier, "TRADER");
        assert_invariants(&c);
    }

    // -----------------------------------------------------------------
    // NEP-141/145 compliance
    // -----------------------------------------------------------------

    #[test]
    fn ft_transfer_requires_registration() {
        let mut c = init();
        register(&mut c, alice(), 1_500);
        // bob is NOT registered
        set(alice(), 2_000);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.ft_transfer(bob(), U128(100), None)
        }));
        assert!(res.is_err(), "unregistered receiver must panic");

        register(&mut c, bob(), 1_500);
        set(alice(), 2_000);
        c.ft_transfer(bob(), U128(100), None);
        assert_eq!(c.ft_balance_of(bob()).0, 99); // 1% tax = 1, half burned
        assert_invariants(&c);
    }

    #[test]
    fn storage_deposit_withdraw_unregister_flow() {
        let mut c = init();

        // below min panics
        set_dep(alice(), 1_500, STORAGE_MIN - 1);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.storage_deposit(None, None)
        }));
        assert!(res.is_err());

        // register at min
        set_dep(alice(), 1_501, STORAGE_MIN);
        let sb = c.storage_deposit(None, None);
        assert_eq!(sb.total.0, STORAGE_MIN);
        assert_eq!(sb.available.0, 0);

        // top-up
        set_dep(alice(), 1_502, 2 * STORAGE_MIN);
        let sb = c.storage_deposit(Some(alice()), Some(false));
        assert_eq!(sb.total.0, 3 * STORAGE_MIN);
        assert_eq!(sb.available.0, 2 * STORAGE_MIN);

        // registration_only on an existing account is a no-op refund
        set_dep(alice(), 1_503, STORAGE_MIN);
        let sb = c.storage_deposit(Some(alice()), Some(true));
        assert_eq!(sb.total.0, 3 * STORAGE_MIN);

        // withdraw all available, keeps min locked
        set(alice(), 1_504);
        let w = c.storage_withdraw(None);
        assert_eq!(w.total.0, STORAGE_MIN);
        assert_eq!(w.available.0, 0);

        // balances non-zero -> unregister must be forced (burns)
        assert!(c.storage_balance_of(alice()).is_some());
        set(alice(), 1_505);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.storage_unregister(None)));
        assert!(res.is_err(), "unregister with balance must panic without force");

        set(alice(), 1_506);
        assert!(c.storage_unregister(Some(true)));
        assert!(c.storage_balance_of(alice()).is_none());
        assert_eq!(c.ft_total_supply().0, 0); // force-unregister burned alice's holdings
        assert_eq!(c.storage_total, 0);
        assert_invariants(&c);
    }

    #[test]
    fn transfer_call_moves_tokens_and_resolves_refund() {
        let mut c = init();
        register(&mut c, alice(), 1_500);
        register(&mut c, bob(), 1_500);
        set(alice(), 2_000);

        let promise = c.ft_transfer_call(bob(), U128(10_000), None, "do something".into());
        drop(promise); // unit-test env: promise chain is mocked, state effect stands
        assert_eq!(c.ft_balance_of(bob()).0, 9_900); // taxed on arrival
        assert_invariants(&c);

        // callback: receiver declares 4_000 unused -> refunded untaxed to sender
        let mut b = VMContextBuilder::new();
        b.current_account_id(contract())
            .predecessor_account_id(contract())
            .block_timestamp(2_100 * 1_000_000_000);
        testing_env_with_promise_results(b.build(), PromiseResult::Successful(b"4000".to_vec()));
        let refunded = c.ft_resolve_transfer(alice(), bob(), U128(10_000)).0;
        assert_eq!(refunded, 4_000);
        assert_eq!(c.ft_balance_of(alice()).0, 1_000_000_000 - 10_000 + 4_000);
        assert_eq!(c.ft_balance_of(bob()).0, 5_900);
        assert_invariants(&c);
    }

    #[test]
    fn resolve_transfer_full_refund_when_receiver_fails() {
        let mut c = init();
        register(&mut c, alice(), 1_500);
        register(&mut c, bob(), 1_500);
        set(alice(), 2_000);
        c.ft_transfer(bob(), U128(10_000), None); // bob has 9_900

        let mut b = VMContextBuilder::new();
        b.current_account_id(contract())
            .predecessor_account_id(contract())
            .block_timestamp(2_100 * 1_000_000_000);
        // failed ft_on_transfer -> refund everything the receiver received
        testing_env_with_promise_results(b.build(), PromiseResult::Failed);
        let refunded = c.ft_resolve_transfer(alice(), bob(), U128(10_000)).0;
        assert_eq!(refunded, 9_900);
        assert_eq!(c.ft_balance_of(bob()).0, 0);
        assert_invariants(&c);
    }

    #[test]
    fn resolve_transfer_direct_call_panics() {
        let mut c = init();
        register(&mut c, alice(), 1_500);
        register(&mut c, bob(), 1_500);
        // called directly (predecessor = alice, no promise result): must panic
        set(alice(), 2_000);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.ft_resolve_transfer(alice(), bob(), U128(10_000))
        }));
        assert!(res.is_err());
    }

    // -----------------------------------------------------------------
    // randomized lifecycle: invariants after every action
    // -----------------------------------------------------------------

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // xorshift-style finalizer: raw LCG low bits are badly correlated
        // (without this the test's roll sequence locks into a 4/5 cycle and
        // never exercises transfers)
        let mut x = *seed;
        x ^= x >> 30;
        x ^= x << 27;
        x ^= x >> 31;
        x
    }

    #[test]
    fn randomized_lifecycle_preserves_invariants() {
        let mut c = init();
        let base = 1_000u64;
        let day = 86_400u64;
        let users = [alice(), bob(), carol()];
        for u in &users {
            register(&mut c, u.clone(), base);
        }
        register(&mut c, resolver(), base);
        register(&mut c, passport(), base);

        set(resolver(), base);
        let e0 = c.create_event("open event".into(), "A".into(), "B".into(), base + 100_000 * day, base + 200_000 * day);
        let e1 = c.create_event("single-sided".into(), "A".into(), "B".into(), base + 100_000 * day, base + 200_000 * day);

        // seed both sides of e0 so it resolves (not voids), and fund the
        // underfunded accounts so transfers/stakes can actually happen
        set(alice(), base);
        c.ft_transfer(bob(), U128(50_000), None);
        c.ft_transfer(carol(), U128(50_000), None);
        set(alice(), base);
        c.stake(e0, 0, U128(100));
        set(bob(), base);
        c.stake(e0, 1, U128(100));

        let mut seed = 0x5EEDu64;
        for i in 0..300u64 {
            let now = base + i * 3_600;
            let roll = lcg(&mut seed) % 8;
            let u1 = users[(lcg(&mut seed) % 3) as usize].clone();
            let u2 = users[(lcg(&mut seed) % 3) as usize].clone();

            // lifecycle events at fixed rounds
            if i == 150 {
                set(resolver(), now);
                c.resolve(e0, 0);
            }
            if i == 180 {
                set(resolver(), now);
                c.resolve(e1, 1); // single side staked -> Void
            }

            set(u1.clone(), now);
            match roll {
                0 => {
                    let amount = (lcg(&mut seed) % 5_000 + 1) as u128;
                    if u1 != u2 && c.ft_balance_of(u1.clone()).0 >= amount {
                        c.ft_transfer(u2.clone(), U128(amount), None);
                    }
                }
                1 => {
                    let amount = (lcg(&mut seed) % 3_000 + 1) as u128;
                    let open = matches!(c.get_event(e0).unwrap().status, Status::Open);
                    if open && c.ft_balance_of(u1.clone()).0 >= amount {
                        let side = match c.get_pick(u1.clone(), e0) {
                            Some(p) => p.side,
                            None => (lcg(&mut seed) % 2) as u8,
                        };
                        c.stake(e0, side, U128(amount));
                    }
                }
                2 => {
                    if let (Some(p), Status::Resolved { winner, .. }) =
                        (c.get_pick(u1.clone(), e0), c.get_event(e0).unwrap().status)
                    {
                        if p.side == winner && !p.claimed {
                            c.claim(e0);
                        }
                    }
                }
                3 => {
                    // alice keeps the second event single-sided on purpose
                    if u1 == alice() {
                        let amount = (lcg(&mut seed) % 2_000 + 1) as u128;
                        let open = matches!(c.get_event(e1).unwrap().status, Status::Open);
                        if open && c.ft_balance_of(alice()).0 >= amount {
                            c.stake(e1, 0, U128(amount));
                        }
                    }
                }
                4 => {
                    if let (Some(p), Status::Void { .. }) =
                        (c.get_pick(u1.clone(), e1), c.get_event(e1).unwrap().status)
                    {
                        if !p.refunded {
                            c.refund(e1);
                        }
                    }
                }
                5 => {
                    let target = users[(lcg(&mut seed) % 3) as usize].clone();
                    let ev = if lcg(&mut seed) % 2 == 0 { e0 } else { e1 };
                    if let Some(p) = c.get_pick(target.clone(), ev) {
                        if !p.settled
                            && !matches!(c.get_event(ev).unwrap().status, Status::Open)
                        {
                            c.settle_for(target, ev);
                        }
                    }
                }
                6 => {
                    // NEP-145: top-up then withdraw
                    set_dep(u1.clone(), now, 1_000);
                    c.storage_deposit(None, None);
                    set(u1.clone(), now);
                    let _ = c.storage_withdraw(None);
                }
                _ => {
                    let amount = (lcg(&mut seed) % 4_000 + 1) as u128;
                    if u1 != u2 && c.ft_balance_of(u1.clone()).0 >= amount {
                        let p = c.ft_transfer_call(u2.clone(), U128(amount), None, "x".into());
                        drop(p);
                    }
                }
            }
            assert_invariants(&c);
            assert!(
                env::account_balance().as_yoctonear() >= c.storage_total,
                "NEAR solvency broken at round {i}"
            );
        }

        // post-loop: sweep after the claim window (permissionless)
        assert!(matches!(
            c.get_event(e0).unwrap().status,
            Status::Resolved { .. }
        ));
        set(carol(), base + 150 * 3_600 + 91 * day);
        c.sweep(e0);
        assert!(c.get_event(e0).unwrap().swept);
        assert_invariants(&c);

        // force-unregister a funded account: burn + NEAR refund, still solvent
        set(carol(), base + 150 * 3_600 + 92 * day);
        let _ = c.storage_unregister(Some(true));
        assert!(c.storage_balance_of(carol()).is_none());
        assert_invariants(&c);

        // the lifecycle produced real log entries (stakes, tax burns, resolution)
        let hist = c.get_history(HISTORY_CAP as u32);
        assert!(hist.len() > 10);
        assert!(hist
            .iter()
            .any(|e| matches!(e.kind, LogKind::Resolve | LogKind::Void)));
    }
}
