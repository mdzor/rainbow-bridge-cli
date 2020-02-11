#[cfg(target_arch = "wasm32")]
use std::io::Cursor;
use borsh::{BorshDeserialize, BorshSerialize};
use eth_types::*;
use near_bindgen::near_bindgen;
use near_bindgen::collections::{Map, Set};

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests;

#[derive(Default, Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct DoubleNodeWithMerkleProof {
    pub dag_nodes: Vec<H512>, // [H512; 2]
    pub proof: Vec<H128>,
}

impl DoubleNodeWithMerkleProof {
    fn truncate_to_h128(arr: H256) -> H128 {
        let mut data = [0u8; 16];
        data.copy_from_slice(&(arr.0).0[16..]);
        H128(data.into())
    }

    fn hash_h128(l: H128, r: H128) -> H128 {
        let mut data = [0u8; 64];
        data[16..32].copy_from_slice(&(l.0).0);
        data[48..64].copy_from_slice(&(r.0).0);
        Self::truncate_to_h128(near_sha256(&data).into())
    }

    pub fn apply_merkle_proof(&self, index: u64) -> H128 {
        let mut data = [0u8; 128];
        data[..64].copy_from_slice(&(self.dag_nodes[0].0).0);
        data[64..].copy_from_slice(&(self.dag_nodes[1].0).0);

        let mut leaf = Self::truncate_to_h128(near_sha256(&data).into());

        for i in 0..self.proof.len() {
            if (index >> i as u64) % 2 == 0 {
                leaf = Self::hash_h128(leaf, self.proof[i]);
            } else {
                leaf = Self::hash_h128(self.proof[i], leaf);
            }
        }
        leaf
    }
}

#[derive(Default, BorshDeserialize, BorshSerialize)]
pub struct HeaderInfo {
    pub total_difficulty: U256,
    pub parent_hash: H256,
    pub number: u64,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct EthBridge {
    dags_start_epoch: u64,
    dags_merkle_roots: Vec<H128>,

    best_header_hash: H256,
    canonical_header_hashes: Map<u64, H256>,

    headers: Map<H256, BlockHeader>,
    infos: Map<H256, HeaderInfo>,

    recent_header_hashes: Map<u64, Set<H256>>,
}

const NUMBER_OF_BLOCKS_FINALITY: u64 = 30;

impl EthBridge {
    pub fn init(dags_start_epoch: u64, dags_merkle_roots: Vec<H128>) -> Self {
        Self {
            dags_start_epoch,
            dags_merkle_roots,

            best_header_hash: Default::default(),
            canonical_header_hashes: Map::new(b"c".to_vec()),

            headers: Map::new(b"h".to_vec()),
            infos: Map::new(b"i".to_vec()),

            recent_header_hashes: Map::new(b"r".to_vec()),
        }
    }

    pub fn initialized(&self) -> bool {
        self.dags_merkle_roots.len() > 0
    }

    pub fn last_block_number(&self) -> u64 {
        self.infos.get(&self.best_header_hash).unwrap_or_default().number
    }

    pub fn dag_merkle_root(&self, epoch: u64) -> H128 {
        self.dags_merkle_roots[(&epoch - self.dags_start_epoch) as usize]
    }

    pub fn block_hash(&self, index: u64) -> Option<H256> {
        self.canonical_header_hashes.get(&index)
    }

    pub fn add_block_header(
        &mut self,
        block_header: Vec<u8>,
        dag_nodes: Vec<DoubleNodeWithMerkleProof>,
    ) {
        let header: BlockHeader = rlp::decode(block_header.as_slice()).unwrap();

        if self.best_header_hash == Default::default() {
            // Submit very first block, can trust relayer
            self.maybe_store_header(header);
            return;
        }

        let header_hash = header.hash.unwrap();
        if self.infos.get(&header_hash).is_some() {
            // The header is already known
            return;
        }

        let prev = self.headers.get(&header.parent_hash).expect("Parent header should be present to add a new header");

        assert!(Self::verify_header(&self, &header, &prev, &dag_nodes), "The new header should be valid");

        self.maybe_store_header(header);
    }
}

impl EthBridge {

    /// Maybe stores a valid header in the contract.
    fn maybe_store_header(&mut self, header: BlockHeader) {
        let best_info = self.infos.get(&self.best_header_hash).unwrap_or_default();
        if best_info.number > header.number + NUMBER_OF_BLOCKS_FINALITY {
            // It's too late to add this block header.
            return;
        }
        let header_hash = header.hash.unwrap();
        self.headers.insert(&header_hash, &header);

        let parent_info = self.infos.get(&header.parent_hash).unwrap_or_default();
        // Have to compute new total difficulty
        let info = HeaderInfo {
            total_difficulty: parent_info.total_difficulty + header.difficulty,
            parent_hash: header.parent_hash.clone(),
            number: header.number,
        };
        self.infos.insert(&header_hash, &info);
        self.add_recent_header_hash(info.number, &header_hash);
        if info.total_difficulty > best_info.total_difficulty ||
            (info.total_difficulty == best_info.total_difficulty && header.difficulty % 2 == U256::default()) {
            // The new header is the tip of the new canonical chain.
            // We need to update hashes of the canonical chain to match the new header.

            // If the new header has a lower number than the previous header, we need to cleaning
            // it going forward.
            if best_info.number > info.number {
                for number in info.number+1..=best_info.number {
                    self.canonical_header_hashes.remove(&number);
                }
            }
            // Replacing the global best header hash.
            self.best_header_hash = header_hash;
            self.canonical_header_hashes.insert(&info.number, &header_hash);

            // Replacing past hashes until we converge into the same parent.
            // Starting from the parent hash.
            let mut number = header.number - 1;
            let mut current_hash = info.parent_hash;
            loop {
                let prev_value = self.canonical_header_hashes.insert(&number, &current_hash);
                // If the current block hash is 0 (unlikely), or the previous hash matches the
                // current hash, then we chains converged and can stop now.
                if number == 0 || prev_value == Some(current_hash) {
                    break;
                }
                // Check if there is an info to get the parent hash
                if let Some(info) = self.infos.get(&current_hash) {
                    current_hash = info.parent_hash;
                } else {
                    break;
                }
                number -= 1;
            }

            self.maybe_gc(best_info.number, info.number);
        }
    }

    /// Removes old headers beyond the finality.
    fn maybe_gc(&mut self, last_best_number: u64, new_best_number: u64) {
        if new_best_number > last_best_number && last_best_number >= NUMBER_OF_BLOCKS_FINALITY {
            for number in last_best_number - NUMBER_OF_BLOCKS_FINALITY..new_best_number - NUMBER_OF_BLOCKS_FINALITY {
                near_bindgen::env::log(format!("Going to GC headers for block number #{}", number).as_bytes());
                if let Some(mut hashes) = self.recent_header_hashes.get(&number) {
                    for hash in hashes.iter() {
                        self.infos.remove(&hash);
                        self.headers.remove(&hash);
                    }
                    hashes.clear();
                    self.recent_header_hashes.remove(&number);
                }
            }
        }
    }

    fn add_recent_header_hash(&mut self, number: u64, hash: &H256) {
        let mut hashes = self.recent_header_hashes.get(&number).unwrap_or_else(|| {
            let mut set_id = Vec::with_capacity(9);
            set_id.extend_from_slice(b"s");
            set_id.extend(number.to_le_bytes().into_iter());
            Set::new(set_id)
        });
        hashes.insert(&hash);
        self.recent_header_hashes.insert(&number, &hashes);
    }

    fn verify_header(
        &self,
        header: &BlockHeader,
        prev: &BlockHeader,
        dag_nodes: &[DoubleNodeWithMerkleProof],
    ) -> bool {
        let (_mix_hash, result) = Self::hashimoto_merkle(
            self,
            &header.partial_hash.unwrap(),
            &header.nonce,
            header.number,
            dag_nodes,
        );

        //
        // See YellowPaper formula (50) in section 4.3.4
        // 1. Simplified difficulty check to conform adjusting difficulty bomb
        // 2. Added condition: header.parent_hash() == prev.hash()
        //
        ethereum_types::U256::from((result.0).0) < ethash::cross_boundary(header.difficulty.0)
            && header.difficulty < header.difficulty * 101 / 100
            && header.difficulty > header.difficulty * 99 / 100
            && header.gas_used <= header.gas_limit
            && header.gas_limit < prev.gas_limit * 1025 / 1024
            && header.gas_limit > prev.gas_limit * 1023 / 1024
            && header.gas_limit >= U256(5000.into())
            && header.timestamp > prev.timestamp
            && header.number == prev.number + 1
            && header.parent_hash == prev.hash.unwrap()
    }

    fn hashimoto_merkle(
        &self,
        header_hash: &H256,
        nonce: &H64,
        block_number: u64,
        nodes: &[DoubleNodeWithMerkleProof],
    ) -> (H256, H256) {
        // Boxed index since ethash::hashimoto gets Fn, but not FnMut
        let index = std::cell::RefCell::new(0);

        // Reuse single Merkle root across all the proofs
        let merkle_root = self.dag_merkle_root((block_number as usize / 30000) as u64);

        let pair = ethash::hashimoto_with_hasher(
            header_hash.0,
            nonce.0,
            ethash::get_full_size(block_number as usize / 30000),
            |offset| {
                let idx = *index.borrow_mut();
                *index.borrow_mut() += 1;

                // Each two nodes are packed into single 128 bytes with Merkle proof
                let node = &nodes[idx / 2];
                if idx % 2 == 0 {
                    // Divide by 2 to adjust offset for 64-byte words instead of 128-byte
                    assert_eq!(merkle_root, node.apply_merkle_proof((offset / 2) as u64));
                };

                // Reverse each 32 bytes for ETHASH compatibility
                let mut data = (node.dag_nodes[idx % 2].0).0;
                data[..32].reverse();
                data[32..].reverse();
                data.into()
            },
            near_keccak256,
            near_keccak512,
        );

        (H256(pair.0), H256(pair.1))
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn init() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let input = near_bindgen::env::input().unwrap();
    let mut c = Cursor::new(&input);
    let dags_start_epoch: u64 = borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    let dags_merkle_roots: Vec<H128> =
        borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    assert_eq!(c.position(), input.len() as u64, "Not all bytes read from input");
    assert!(near_bindgen::env::state_read::<EthBridge>().is_none(), "Already initialized");
    let contract = EthBridge::init(dags_start_epoch, dags_merkle_roots);
    near_bindgen::env::state_write(&contract);
}
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn initialized() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let contract: EthBridge = near_bindgen::env::state_read().unwrap();
    let result = contract.initialized();
    let result = result.try_to_vec().unwrap();
    near_bindgen::env::value_return(&result);
}
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn last_block_number() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let contract: EthBridge = near_bindgen::env::state_read().unwrap();
    let result = contract.last_block_number();
    let result = result.try_to_vec().unwrap();
    near_bindgen::env::value_return(&result);
}
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn dag_merkle_root() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let input = near_bindgen::env::input().unwrap();
    let mut c = Cursor::new(&input);
    let epoch: u64 = borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    assert_eq!(c.position(), input.len() as u64, "Not all bytes read from input");
    let contract: EthBridge = near_bindgen::env::state_read().unwrap();
    let result = contract.dag_merkle_root(epoch);
    let result = result.try_to_vec().unwrap();
    near_bindgen::env::value_return(&result);
}
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn block_hash() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let input = near_bindgen::env::input().unwrap();
    let mut c = Cursor::new(&input);
    let index: u64 = borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    assert_eq!(c.position(), input.len() as u64, "Not all bytes read from input");
    let contract: EthBridge = near_bindgen::env::state_read().unwrap();
    let result = contract.block_hash(index);
    let result = result.try_to_vec().unwrap();
    near_bindgen::env::value_return(&result);
}
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn add_block_header() {
    near_bindgen::env::setup_panic_hook();
    near_bindgen::env::set_blockchain_interface(Box::new(near_blockchain::NearBlockchain {}));
    let input = near_bindgen::env::input().unwrap();
    let mut c = Cursor::new(&input);
    let block_header: Vec<u8> = borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    let dag_nodes: Vec<DoubleNodeWithMerkleProof> =
        borsh::BorshDeserialize::deserialize(&mut c).unwrap();
    assert_eq!(c.position(), input.len() as u64, "Not all bytes read from input");
    let mut contract: EthBridge = near_bindgen::env::state_read().unwrap();
    contract.add_block_header(block_header, dag_nodes);
    near_bindgen::env::state_write(&contract);
}
