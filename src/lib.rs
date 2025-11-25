pub mod protos {
    pub mod consensus {
        pub mod v1 {
            include!(concat!(env!("OUT_DIR"), "/consensus.v1.rs"));
        }
    }
}

pub mod consensus;
pub mod error;
pub mod events;
pub mod scope;
pub mod service;
pub mod session;
pub mod stats;
pub mod storage;
pub mod utils;
