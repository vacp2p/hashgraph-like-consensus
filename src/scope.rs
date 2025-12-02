use std::{fmt::Debug, hash::Hash};

/// A scope groups related proposals together.
///
/// Think of it like a namespace or category. For example, you might use a scope as group of users.
/// Any type that can be used as a group of users can be used as a scope.
pub trait ConsensusScope: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

impl<T> ConsensusScope for T where T: Clone + Eq + Hash + Send + Sync + Debug + 'static {}

/// A simple string-based scope identifier.
pub type ScopeID = String;
