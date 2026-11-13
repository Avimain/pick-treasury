//! Prediction Passport — soulbound NEP-171 NFT that records your $PICK
//! predictions and gates the tier claim bonus (the coin checks
//! `has_passport`, registered by this contract).
//!
//! - One passport per account, minted free, non-transferable (soulbound).
//! - The coin mirrors picks/results here via `record_pick` / `record_result`
//!   (fire-and-forget). The coin stays authoritative; if a mirror write was
//!   missed, `repair()` re-registers the perk and results still land as they
//!   settle. Documented honestly in the README.

use std::collections::HashMap;

use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, AccountId, Gas, NearToken, PanicOnDefault};

// ---------------------------------------------------------------------------
// storage types
// ---------------------------------------------------------------------------

#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct Record {
    pub event_id: u64,
    /// `None` when a result landed without a mirrored pick (minted late).
    pub side: Option<u8>,
    pub amount: u128,
    /// None = pending, 0 = loss, 1 = win, 2 = void/no-contest.
    pub outcome: Option<u8>,
    pub payout: u128,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct NftMetadata {
    pub spec: String,
    pub name: String,
    pub symbol: String,
    pub icon: Option<String>,
    pub base_uri: Option<String>,
    pub reference: Option<String>,
    pub reference_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// view / standard types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct RecordView {
    pub event_id: u64,
    pub side: Option<u8>,
    pub amount: U128,
    pub outcome: Option<u8>,
    pub payout: U128,
}

impl From<&Record> for RecordView {
    fn from(r: &Record) -> Self {
        Self {
            event_id: r.event_id,
            side: r.side,
            amount: U128(r.amount),
            outcome: r.outcome,
            payout: U128(r.payout),
        }
    }
}

#[derive(Serialize, Default, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Summary {
    pub picks: u32,
    pub resolved: u32,
    pub wins: u32,
    pub losses: u32,
    pub voids: u32,
    pub streak: u64,
    pub best: u64,
    pub tier: String,
}

#[derive(Serialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct TokenMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub media: Option<String>,
    pub media_hash: Option<String>,
    pub copies: Option<u64>,
    pub issued_at: Option<String>,
    pub expires_at: Option<String>,
    pub starts_at: Option<String>,
    pub updated_at: Option<String>,
    pub extra: Option<String>,
    pub reference: Option<String>,
    pub reference_hash: Option<String>,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Token {
    pub token_id: String,
    pub owner_id: AccountId,
    pub metadata: Option<TokenMetadata>,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
struct RegisterArgs {
    user: AccountId,
}

// ---------------------------------------------------------------------------
// contract
// ---------------------------------------------------------------------------

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Passport {
    /// The $PICK contract; sole writer of records, target of registration.
    pub coin: AccountId,
    pub trader_wins: u32,
    pub oracle_wins: u32,
    pub meta: NftMetadata,
    pub minted: HashMap<AccountId, bool>,
    pub records: HashMap<AccountId, Vec<Record>>,
}

#[near]
impl Passport {
    #[init]
    pub fn new(coin: AccountId, trader_wins: u32, oracle_wins: u32, meta: NftMetadata) -> Self {
        assert!(coin != env::current_account_id(), "coin = self");
        assert!(trader_wins > 0 && oracle_wins >= trader_wins, "bad tiers");
        assert!(!meta.symbol.is_empty(), "empty symbol");
        Self {
            coin,
            trader_wins,
            oracle_wins,
            meta,
            minted: HashMap::new(),
            records: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------
    // mint (soulbound, one per account, free)
    // -----------------------------------------------------------------

    pub fn mint(&mut self) {
        assert_eq!(env::attached_deposit(), NearToken::from_yoctonear(0), "mint is free");
        let user = env::predecessor_account_id();
        assert!(!self.minted.contains_key(&user), "already minted");
        self.minted.insert(user.clone(), true);
        self.records.insert(user.clone(), Vec::new());
        self.register(user);
    }

    /// Re-send registration to the coin (idempotent) if the original
    /// fire-and-forget receipt failed — restores the tier perk.
    pub fn repair(&mut self) {
        let user = env::predecessor_account_id();
        assert!(self.minted.contains_key(&user), "not minted");
        self.register(user);
    }

    fn register(&self, user: AccountId) {
        let args = near_sdk::serde_json::to_vec(&RegisterArgs { user }).expect("serialize");
        let _ = env::promise_create(
            self.coin.clone(),
            "register_passport",
            &args,
            NearToken::from_yoctonear(0),
            Gas::from_tgas(15),
        );
    }

    // -----------------------------------------------------------------
    // record mirroring (coin only)
    // -----------------------------------------------------------------

    pub fn record_pick(&mut self, user: AccountId, event_id: u64, side: u8, amount: U128) {
        assert_eq!(env::predecessor_account_id(), self.coin, "only coin");
        assert!(side < 2, "bad side");
        if !self.minted.contains_key(&user) {
            return; // no passport — coin record remains authoritative
        }
        let recs = self.records.entry(user).or_default();
        if let Some(r) = recs.iter_mut().find(|r| r.event_id == event_id) {
            assert_eq!(r.side, Some(side), "side mismatch");
            r.amount += amount.0;
        } else {
            assert!(recs.len() < 10_000, "record cap");
            recs.push(Record {
                event_id,
                side: Some(side),
                amount: amount.0,
                outcome: None,
                payout: 0,
            });
        }
    }

    pub fn record_result(&mut self, user: AccountId, event_id: u64, outcome: u8, payout: U128) {
        assert_eq!(env::predecessor_account_id(), self.coin, "only coin");
        assert!(outcome <= 2, "bad outcome");
        let Some(recs) = self.records.get_mut(&user) else {
            return; // no passport
        };
        match recs.iter_mut().find(|r| r.event_id == event_id) {
            Some(r) => {
                if r.outcome.is_none() {
                    r.outcome = Some(outcome);
                    r.payout = payout.0;
                } else if outcome == 1 && r.outcome == Some(1) && r.payout == 0 && payout.0 > 0 {
                    // win was mirrored at settle time with payout 0; the claim
                    // receipt fills in the actual amount (order-independent:
                    // if the claim landed first, this condition is false)
                    r.payout = payout.0;
                }
            }
            None => {
                // minted after staking — keep the result, mark the pick as imported
                recs.push(Record {
                    event_id,
                    side: None,
                    amount: 0,
                    outcome: Some(outcome),
                    payout: payout.0,
                });
            }
        }
    }

    // -----------------------------------------------------------------
    // NEP-171 (soulbound subset)
    // -----------------------------------------------------------------

    pub fn nft_token(&self, token_id: String) -> Option<Token> {
        let owner: AccountId = token_id.parse().ok()?;
        if !self.minted.contains_key(&owner) {
            return None;
        }
        let summary = self.summary_of(&owner);
        let description = format!(
            "Soulbound record of $PICK predictions for {}. {} picks, {} resolved — {}W/{}L, streak {}, best {}, tier {}.",
            owner, summary.picks, summary.resolved, summary.wins, summary.losses,
            summary.streak, summary.best, summary.tier
        );
        let extra = near_sdk::serde_json::json!({
            "picks": summary.picks,
            "resolved": summary.resolved,
            "wins": summary.wins,
            "losses": summary.losses,
            "voids": summary.voids,
            "streak": summary.streak,
            "best": summary.best,
            "tier": summary.tier,
        })
        .to_string();
        Some(Token {
            token_id,
            owner_id: owner,
            metadata: Some(TokenMetadata {
                title: Some(self.meta.name.clone()),
                description: Some(description),
                media: self.meta.icon.clone(),
                media_hash: None,
                copies: Some(1),
                issued_at: None,
                expires_at: None,
                starts_at: None,
                updated_at: None,
                extra: Some(extra),
                reference: None,
                reference_hash: None,
            }),
        })
    }

    pub fn nft_metadata(&self) -> NftMetadata {
        self.meta.clone()
    }

    pub fn nft_transfer(
        &mut self,
        _receiver_id: AccountId,
        _token_id: Option<String>,
        _approval_id: Option<u64>,
        _memo: Option<String>,
    ) {
        panic!("soulbound: passports cannot be transferred");
    }

    pub fn nft_transfer_call(
        &mut self,
        _receiver_id: AccountId,
        _token_id: String,
        _approval_id: Option<u64>,
        _memo: Option<String>,
        _msg: String,
    ) -> near_sdk::PromiseOrValue<bool> {
        panic!("soulbound: passports cannot be transferred");
    }

    pub fn nft_resolve_transfer(
        &mut self,
        _previous_owner_id: AccountId,
        _token_id: String,
        _refund_account_id: Option<AccountId>,
    ) -> bool {
        false // transfers always fail up-front; nothing to resolve
    }

    pub fn nft_total_supply(&self) -> U128 {
        U128(self.minted.len() as u128)
    }

    pub fn nft_supply_for_owner(&self, account_id: AccountId) -> U128 {
        U128(u8::from(self.minted.contains_key(&account_id)) as u128)
    }

    // -----------------------------------------------------------------
    // views
    // -----------------------------------------------------------------

    pub fn get_records(&self, user: AccountId) -> Vec<RecordView> {
        self.records
            .get(&user)
            .map(|r| r.iter().map(RecordView::from).collect())
            .unwrap_or_default()
    }

    pub fn get_summary(&self, user: AccountId) -> Option<Summary> {
        if !self.minted.contains_key(&user) {
            return None;
        }
        Some(self.summary_of(&user))
    }

    pub fn is_minted(&self, user: AccountId) -> bool {
        self.minted.contains_key(&user)
    }

    fn summary_of(&self, user: &AccountId) -> Summary {
        let empty = Vec::new();
        let recs = self.records.get(user).unwrap_or(&empty);
        let mut s = Summary {
            picks: recs.len() as u32,
            ..Summary::default()
        };
        s.tier = "-".to_string();
        for r in recs {
            match r.outcome {
                Some(1) => {
                    s.wins += 1;
                    s.resolved += 1;
                }
                Some(0) => {
                    s.losses += 1;
                    s.resolved += 1;
                }
                Some(2) => {
                    s.voids += 1;
                    s.resolved += 1;
                }
                Some(_) | None => {}
            }
        }
        // trailing-win streak (voids and pending picks don't break it)
        for r in recs.iter().rev() {
            match r.outcome {
                Some(1) => s.streak += 1,
                Some(0) => break,
                _ => continue,
            }
        }
        // longest win run over resolved picks
        let mut run = 0u64;
        for r in recs.iter().rev() {
            match r.outcome {
                Some(1) => {
                    run += 1;
                    if run > s.best {
                        s.best = run;
                    }
                }
                Some(0) => run = 0,
                _ => continue,
            }
        }
        s.tier = if s.wins >= self.oracle_wins {
            "ORACLE".to_string()
        } else if s.wins >= self.trader_wins {
            "TRADER".to_string()
        } else {
            "ROOKIE".to_string()
        };
        s
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

    fn coin_acct() -> AccountId {
        accounts(0)
    }
    fn user() -> AccountId {
        accounts(1)
    }
    fn passport_acct() -> AccountId {
        accounts(3)
    }

    fn set(pred: AccountId) {
        let mut b = VMContextBuilder::new();
        b.current_account_id(passport_acct())
            .predecessor_account_id(pred);
        testing_env!(b.build());
    }

    fn meta() -> NftMetadata {
        NftMetadata {
            spec: "nft-1.0.0".into(),
            name: "Prediction Passport".into(),
            symbol: "PASS".into(),
            icon: Some("data:image/svg+xml;base64,PHN2Zy8+".into()),
            base_uri: None,
            reference: None,
            reference_hash: None,
        }
    }

    fn init() -> Passport {
        set(coin_acct());
        Passport::new(coin_acct(), 2, 5, meta())
    }

    #[test]
    fn mint_mirrors_and_summary() {
        let mut c = init();
        set(user());
        c.mint();
        assert!(c.is_minted(user()));

        // coin-side writes
        set(coin_acct());
        c.record_pick(user(), 7, 0, U128(500));
        c.record_pick(user(), 7, 0, U128(100)); // top-up, same pick
        c.record_pick(user(), 8, 1, U128(300));
        c.record_result(user(), 7, 1, U128(600));

        let recs = c.get_records(user());
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].amount.0, 600);
        assert_eq!(recs[0].outcome, Some(1));

        let s = c.get_summary(user()).unwrap();
        assert_eq!((s.picks, s.resolved, s.wins), (2, 1, 1));
        assert_eq!(s.streak, 1);
        assert_eq!(s.tier, "ROOKIE"); // 1 win < trader_wins(2)

        // nft_token exposes the live record
        let tok = c.nft_token(user().to_string()).expect("token exists");
        assert_eq!(tok.owner_id, user());
        let extra = tok.metadata.unwrap().extra.unwrap();
        assert!(extra.contains("\"wins\":1"));
    }

    #[test]
    fn soulbound_transfer_always_panics() {
        let mut c = init();
        set(user());
        c.mint();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            c.nft_transfer(accounts(2), None, None, None)
        }));
        assert!(res.is_err());
        assert!(c.is_minted(user()), "ownership unchanged after failed transfer");
    }

    #[test]
    fn records_ignored_before_mint_but_results_land_late() {
        let mut c = init();
        // coin writes for a user who has no passport yet -> no-op
        set(coin_acct());
        c.record_pick(user(), 1, 0, U128(500));
        assert!(c.get_records(user()).is_empty());

        // user mints later; a result for the earlier pick still lands
        set(user());
        c.mint();
        set(coin_acct());
        c.record_result(user(), 1, 1, U128(550));
        let recs = c.get_records(user());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].side, None); // imported result
        assert_eq!(recs[0].outcome, Some(1));
    }

    #[test]
    fn win_mirrored_at_settle_then_payout_filled_by_claim() {
        let mut c = init();
        set(user());
        c.mint();

        // coin mirrors the pick, then the win at settle time (payout unknown yet)
        set(coin_acct());
        c.record_pick(user(), 9, 0, U128(500));
        c.record_result(user(), 9, 1, U128(0));
        assert_eq!(c.get_records(user())[0].outcome, Some(1));
        assert_eq!(c.get_records(user())[0].payout.0, 0);

        // claim receipt fills the amount; a late duplicate zero-payout is ignored
        c.record_result(user(), 9, 1, U128(600));
        c.record_result(user(), 9, 1, U128(0));
        assert_eq!(c.get_records(user())[0].payout.0, 600);

        // and the summary counts it as a win with streak 1
        let s = c.get_summary(user()).unwrap();
        assert_eq!((s.wins, s.streak), (1, 1));
    }
}
