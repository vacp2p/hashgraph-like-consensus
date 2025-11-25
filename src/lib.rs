pub mod protos {
    pub mod consensus {
        pub mod v1 {
            include!(concat!(env!("OUT_DIR"), "/consensus.v1.rs"));
        }
    }
}

pub mod service;
pub mod error;
pub mod utils;