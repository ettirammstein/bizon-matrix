use near_sdk::{
    near_bindgen, env, AccountId, Promise, PanicOnDefault, BorshStorageKey,
};
use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::UnorderedMap;
use near_sdk::json_types::U128;

pub type BizonId = String;

#[derive(BorshStorageKey, BorshSerialize)]
pub enum StorageKey {
    Players,
    MatrixFill,
    IdToAccount,
    AccountToId,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
pub struct Player {
    pub bizon_id: BizonId,
    pub referrer: Option<AccountId>,
    pub join_ts: u64,
    pub level: u8,
    pub cycles: u32,
    pub pending_balance: u128,
    pub reinvest_rate: u8,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct BizonMatrix {
    pub players: UnorderedMap<AccountId, Player>,
    pub matrix_fill: UnorderedMap<AccountId, u8>,
    pub id_to_account: UnorderedMap<BizonId, AccountId>,
    pub account_to_id: UnorderedMap<AccountId, BizonId>,
    pub next_id: u64,
    pub daily_pool: u128,
    pub monthly_pool: u128,
    pub yearly_pool: u128,
    pub global_pool: u128,
    pub total_players: u64,
    pub last_daily_ts: u64,
    pub last_monthly_ts: u64,
    pub last_yearly_ts: u64,
    pub owner_id: AccountId,
}

#[near_bindgen]
impl BizonMatrix {
    #[init]
    pub fn new(owner_id: AccountId) -> Self {
        assert!(!env::state_exists(), "Already initialized");
        Self {
            players: UnorderedMap::new(StorageKey::Players),
            matrix_fill: UnorderedMap::new(StorageKey::MatrixFill),
            id_to_account: UnorderedMap::new(StorageKey::IdToAccount),
            account_to_id: UnorderedMap::new(StorageKey::AccountToId),
            next_id: 1,
            daily_pool: 0,
            monthly_pool: 0,
            yearly_pool: 0,
            global_pool: 0,
            total_players: 0,
            last_daily_ts: 0,
            last_monthly_ts: 0,
            last_yearly_ts: 0,
            owner_id,
        }
    }

    #[payable]
    pub fn join(&mut self, ref_raw: Option<String>) {
        let caller = env::predecessor_account_id();
        let deposit = env::attached_deposit();
        let one_near: u128 = 1_000_000_000_000_000_000_000_000;
        assert_eq!(deposit, one_near, "Need exactly 1 NEAR to join");

        let bizon_id = self.ensure_bizon_id(&caller);
        self.split_deposit(one_near);

        if !self.players.contains_key(&caller) {
            let ref_acc = ref_raw
                .as_ref()
                .and_then(|r| self.resolve_ref(r))
                .filter(|acc| acc != &caller);

            let p = Player {
                bizon_id: bizon_id.clone(),
                referrer: ref_acc,
                join_ts: env::block_timestamp(),
                level: 0,
                cycles: 0,
                pending_balance: 0,
                reinvest_rate: 0,
            };
            self.players.insert(&caller, &p);
            self.matrix_fill.insert(&caller, &0);
            self.total_players += 1;
        }

        let emptiest_owner = self.find_emptiest_matrix();
        self.apply_spill(&emptiest_owner);
    }

    pub fn set_reinvest_rate(&mut self, rate: u8) {
        assert!(rate <= 100, "Rate must be 0-100");
        let caller = env::predecessor_account_id();
        if let Some(mut p) = self.players.get(&caller) {
            p.reinvest_rate = rate;
            self.players.insert(&caller, &p);
        }
    }

    fn split_deposit(&mut self, one_near: u128) {
        let daily = one_near * 90 / 100;
        let monthly = one_near * 9 / 100;
        let yearly = one_near * 1 / 100;

        self.daily_pool += daily;
        self.monthly_pool += monthly;
        self.yearly_pool += yearly;

        let used = daily + monthly + yearly;
        if one_near > used {
            self.global_pool += one_near - used;
        }
    }

    fn ensure_bizon_id(&mut self, account: &AccountId) -> BizonId {
        if let Some(id) = self.account_to_id.get(account) {
            return id;
        }
        let id_str = format!("ID{}", self.next_id);
        self.next_id += 1;
        self.id_to_account.insert(&id_str, account);
        self.account_to_id.insert(account, &id_str);
        id_str
    }

    fn resolve_ref(&self, raw: &str) -> Option<AccountId> {
        if raw.starts_with("ID") {
            if let Some(acc) = self.id_to_account.get(&raw.to_string()) {
                return Some(acc);
            }
        }

        if raw.ends_with(".tg") {
            return None;
        }

        if raw.ends_with(".near") || raw.ends_with(".testnet") {
            return raw.parse().ok();
        }

        None
    }

    fn find_emptiest_matrix(&self) -> AccountId {
        let mut min_fill: u8 = u8::MAX;
        let mut candidate: Option<AccountId> = None;

        for (acc, fill) in self.matrix_fill.iter() {
            if *fill < min_fill {
                min_fill = *fill;
                candidate = Some(acc);
            }
        }

        candidate.unwrap_or_else(|| self.owner_id.clone())
    }

    fn apply_spill(&mut self, owner: &AccountId) {
        let mut fill = self.matrix_fill.get(owner).unwrap_or(0);
        if fill < 10 {
            fill += 1;
            self.matrix_fill.insert(owner, &fill);
        }

        if fill >= 10 {
            self.on_matrix_full(owner.clone());
        }
    }

    fn on_matrix_full(&mut self, owner: AccountId) {
        if let Some(mut p) = self.players.get(&owner) {
            p.level += 1;
            if p.level >= 10 {
                p.cycles += 1;
                p.level = 0;
            }
            self.players.insert(&owner, &p);
            self.matrix_fill.insert(&owner, &0);

            let emptiest = self.find_emptiest_matrix();
            if emptiest != owner {
                self.apply_spill(&emptiest);
            }
        }
    }

    pub fn distribute_daily(&mut self) {
        let now = env::block_timestamp();
        let day_ns: u64 = 86_400_000_000_000;
        
        assert!(self.total_players > 0, "No players");
        
        if now < self.last_daily_ts + day_ns {
            return;
        }
        
        if self.daily_pool == 0 {
            self.last_daily_ts = now;
            return;
        }

        let share = self.daily_pool / self.total_players as u128;
        if share == 0 {
            self.global_pool += self.daily_pool;
            self.daily_pool = 0;
            self.last_daily_ts = now;
            return;
        }

        let mut accounts = Vec::new();
        for (acc, _) in self.players.iter() {
            accounts.push(acc);
        }

        for acc in accounts {
            if let Some(mut p) = self.players.get(&acc) {
                p.pending_balance += share;
                self.players.insert(&acc, &p);
            }
        }

        self.daily_pool = 0;
        self.last_daily_ts = now;
    }

    pub fn claim_all(&mut self) -> U128 {
        let caller = env::predecessor_account_id();
        if let Some(mut p) = self.players.get(&caller) {
            let amount = p.pending_balance;
            assert!(amount > 0, "Nothing to claim");

            p.pending_balance = 0;
            self.players.insert(&caller, &p);
            Promise::new(caller).transfer(amount);
            U128(amount)
        } else {
            U128(0)
        }
    }

    pub fn get_my_profile(&self) -> Option<(BizonId, u8, u32, u8, U128)> {
        let caller = env::predecessor_account_id();
        self.players.get(&caller).map(|p| {
            let fill = self.matrix_fill.get(&caller).unwrap_or(0);
            (p.bizon_id, p.level, p.cycles, fill, U128(p.pending_balance))
        })
    }

    pub fn get_my_id(&self) -> Option<BizonId> {
        let caller = env::predecessor_account_id();
        self.account_to_id.get(&caller)
    }

    pub fn disable_owner(&mut self) {
        assert_eq!(env::predecessor_account_id(), self.owner_id, "Not owner");
        self.owner_id = "system".parse().unwrap();
    }
}
