use std::{fmt::Debug, hash::Hash};

/// Marker trait describing a namespace that groups consensus sessions.
///
/// Any type that is clonable, hashable, and thread-safe can automatically act
/// as a scope for the consensus service.
pub trait ConsensusScope: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

impl<T> ConsensusScope for T where T: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

/// Default scope type used when working with simple string-based identifiers.
pub type GroupId = String;
