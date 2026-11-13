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
use near_sdk::{env, near, AccountId, Gas, NearToken, PanicOnDefault};

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
        }
    }

    // -----------------------------------------------------------------
    // NEP-141 fungible token (subset)
    // -----------------------------------------------------------------

    pub fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        assert_eq!(env::attached_deposit(), NearToken::from_yoctonear(0), "no deposit expected");
        assert!(amount.0 > 0, "zero amount");
        let sender = env::predecessor_account_id();
        self.internal_transfer(&sender, &receiver_id, amount.0, true);
        if let Some(m) = memo {
            env::log_str(&format!("Memo: {} ({} -> {})", m, sender, receiver_id));
        }
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

    fn internal_transfer(&mut self, from: &AccountId, to: &AccountId, amount: u128, taxed: bool) {
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
                } else {
                    st.losses += 1;
                    st.streak = 0;
                }
                if !correct {
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
    use near_sdk::test_utils::{accounts, VMContextBuilder};
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
}
