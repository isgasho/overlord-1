use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use lru_cache::LruCache;
use rlp::Encodable;
use serde::{Deserialize, Serialize};
use serde_json;

use overlord::types::Node;
use overlord::{Wal, WalInfo};

use super::primitive::Block;
use super::utils::{create_alive_nodes, gen_random_bytes};

pub const RECORD_TMP_FILE: &str = "./tests/integration_tests/test.json";

pub struct MockWal {
    inner: Mutex<Option<Bytes>>,
}

impl MockWal {
    pub fn new() -> MockWal {
        MockWal {
            inner: Mutex::new(None),
        }
    }
}

#[async_trait]
impl Wal for MockWal {
    async fn save(&self, info: Bytes) -> Result<(), Box<dyn Error + Send>> {
        *self.inner.lock().unwrap() = Some(info);
        Ok(())
    }

    async fn load(&self) -> Result<Option<Bytes>, Box<dyn Error + Send>> {
        Ok(self.inner.lock().unwrap().as_ref().cloned())
    }
}

pub struct Record {
    pub node_record:   Vec<Node>,
    pub alive_record:  Mutex<Vec<Node>>,
    pub wal_record:    HashMap<Bytes, Arc<MockWal>>,
    pub commit_record: Arc<Mutex<LruCache<u64, Bytes>>>,
    pub height_record: Arc<Mutex<HashMap<Bytes, u64>>>,
    pub interval:      u64,
}

impl Record {
    pub fn new(num: usize, interval: u64) -> Record {
        let node_record: Vec<Node> = (0..num).map(|_| Node::new(gen_random_bytes())).collect();
        let alive_record = Mutex::new(create_alive_nodes(node_record.clone()));
        let wal_record: HashMap<Bytes, Arc<MockWal>> = (0..num)
            .map(|i| {
                (
                    node_record.get(i).unwrap().address.clone(),
                    Arc::new(MockWal::new()),
                )
            })
            .collect();
        let commit_record: Arc<Mutex<LruCache<u64, Bytes>>> =
            Arc::new(Mutex::new(LruCache::new(10)));
        commit_record.lock().unwrap().insert(0, gen_random_bytes());
        let height_record: Arc<Mutex<HashMap<Bytes, u64>>> = Arc::new(Mutex::new(
            (0..num)
                .map(|i| (node_record.get(i).unwrap().address.clone(), 0))
                .collect(),
        ));

        Record {
            node_record,
            alive_record,
            wal_record,
            commit_record,
            height_record,
            interval,
        }
    }

    fn to_wal(&self) -> RecordForWal {
        let node_record = self.node_record.clone();
        let alive_record = self.alive_record.lock().unwrap().clone();
        let wal_record: Vec<TupleWalRecord> = self
            .wal_record
            .iter()
            .map(|(name, wal)| {
                TupleWalRecord(
                    name.clone(),
                    wal.inner
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|wal| rlp::decode(&wal).unwrap()),
                )
            })
            .collect();
        let commit_record: Vec<TupleCommitRecord> = self
            .commit_record
            .lock()
            .unwrap()
            .iter()
            .map(|(height, commit_hash)| TupleCommitRecord(*height, commit_hash.clone()))
            .collect();
        let height_record: Vec<TupleHeightRecord> = self
            .height_record
            .lock()
            .unwrap()
            .iter()
            .map(|(name, height)| TupleHeightRecord(name.clone(), *height))
            .collect();
        let interval = self.interval;
        RecordForWal {
            node_record,
            alive_record,
            wal_record,
            commit_record,
            height_record,
            interval,
        }
    }

    pub fn update_alive(&self) -> Vec<Node> {
        let alive_record = create_alive_nodes(self.node_record.clone());
        *self.alive_record.lock().unwrap() = alive_record.clone();
        alive_record
    }

    pub fn save(&self, filename: &str) {
        let file = File::create(filename).unwrap();
        let record_for_wal = self.to_wal();
        serde_json::to_writer_pretty(file, &record_for_wal).unwrap();
    }

    pub fn load(filename: &str) -> Record {
        let file = File::open(filename).unwrap();
        let reader = BufReader::new(file);
        let record_for_wal: RecordForWal = serde_json::from_reader(reader).unwrap();
        record_for_wal.to_record()
    }
}

#[derive(Serialize, Deserialize)]
struct TupleWalRecord(
    #[serde(with = "overlord::serde_hex")] Bytes,
    Option<WalInfo<Block>>,
);

#[derive(Serialize, Deserialize, Clone)]
struct TupleCommitRecord(u64, #[serde(with = "overlord::serde_hex")] Bytes);

#[derive(Serialize, Deserialize, Clone)]
struct TupleHeightRecord(#[serde(with = "overlord::serde_hex")] Bytes, u64);

#[derive(Serialize, Deserialize)]
struct RecordForWal {
    node_record:   Vec<Node>,
    alive_record:  Vec<Node>,
    wal_record:    Vec<TupleWalRecord>,
    commit_record: Vec<TupleCommitRecord>,
    height_record: Vec<TupleHeightRecord>,
    interval:      u64,
}

impl RecordForWal {
    fn to_record(&self) -> Record {
        let node_record = self.node_record.clone();
        let alive_record = Mutex::new(self.alive_record.clone());
        let wal_record: HashMap<Bytes, Arc<MockWal>> = self
            .wal_record
            .iter()
            .map(|TupleWalRecord(name, wal)| {
                (
                    name.clone(),
                    Arc::new(MockWal {
                        inner: Mutex::new(wal.as_ref().map(|wal| Bytes::from(wal.rlp_bytes()))),
                    }),
                )
            })
            .collect();
        let mut commit_record: LruCache<u64, Bytes> = LruCache::new(10);
        for TupleCommitRecord(height, commit_hash) in self.commit_record.clone() {
            commit_record.insert(height, commit_hash);
        }
        let height_record: HashMap<Bytes, u64> = self
            .height_record
            .iter()
            .map(|TupleHeightRecord(name, height)| (name.clone(), *height))
            .collect();
        Record {
            node_record,
            alive_record,
            wal_record,
            commit_record: Arc::new(Mutex::new(commit_record)),
            height_record: Arc::new(Mutex::new(height_record)),
            interval: self.interval,
        }
    }
}
