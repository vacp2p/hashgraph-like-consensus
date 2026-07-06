use std::{fmt::Debug, hash::Hash};

/// A scope groups related proposals together.
///
/// Think of it like a namespace or category. For example, you might use a scope as group of users.
/// The trait is blanket-implemented: any `Clone + Eq + Hash + Send + Sync + Debug + 'static`
/// type works as a scope key — `String`, `Vec<u8>`, `[u8; 32]`, integers, or a
/// custom key type of your own.
pub trait ConsensusScope: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

impl<T> ConsensusScope for T where T: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

/// A simple string-based scope identifier, used by
/// [`DefaultConsensusService`](crate::service::DefaultConsensusService).
/// Integrations keyed by raw bytes can use `Vec<u8>` (or any other
/// [`ConsensusScope`] type) directly in their `ConsensusService` /
/// `ConsensusStorage` type parameters.
pub type ScopeID = String;
