use crate::error::ConsensusError;
use crate::session::ConsensusConfig;

/// Network type determines how rounds and votes are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkType {
    /// Gossipsub network: 2 rounds, multiple votes can be in round 2
    Gossipsub,
    /// P2P network: dynamically calculated max rounds (default is ceil(2n/3)),
    /// each vote increments the round number
    P2P,
}

/// Scope-level configuration that applies to all proposals in a scope.
///
/// This provides default settings for proposals created in a scope.
/// Individual proposals can override these defaults if needed.
#[derive(Debug, Clone)]
pub struct ScopeConfig {
    /// Network type: P2P or Gossipsub
    pub network_type: NetworkType,
    /// Default consensus threshold (e.g., 2/3 = 0.667)
    pub default_consensus_threshold: f64,
    /// Default timeout for proposals in this scope (seconds)
    pub default_timeout: u64,
    /// Default liveness criteria (how silent peers are counted)
    pub default_liveness_criteria_yes: bool,
    /// Optional: Max rounds override (if None, uses network_type defaults)
    pub max_rounds_override: Option<u32>,
}

impl Default for ScopeConfig {
    fn default() -> Self {
        Self {
            network_type: NetworkType::Gossipsub,
            default_consensus_threshold: 2.0 / 3.0,
            default_timeout: 60,
            default_liveness_criteria_yes: true,
            max_rounds_override: None,
        }
    }
}

impl ScopeConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), ConsensusError> {
        crate::utils::validate_threshold(self.default_consensus_threshold)?;
        crate::utils::validate_timeout(self.default_timeout)?;
        // Allow max_rounds_override = Some(0) only for P2P networks (triggers dynamic calculation)
        // For Gossipsub networks, max_rounds_override must be greater than 0
        if let Some(max_rounds) = self.max_rounds_override
            && max_rounds == 0
            && self.network_type == NetworkType::Gossipsub
        {
            return Err(ConsensusError::InvalidProposalConfiguration(
                "max_rounds_override must be greater than 0 for Gossipsub networks".to_string(),
            ));
        }
        Ok(())
    }
}

impl From<NetworkType> for ScopeConfig {
    fn from(network_type: NetworkType) -> Self {
        match network_type {
            NetworkType::Gossipsub => Self {
                network_type: NetworkType::Gossipsub,
                default_consensus_threshold: 2.0 / 3.0,
                default_timeout: 60,
                default_liveness_criteria_yes: true,
                max_rounds_override: None,
            },
            NetworkType::P2P => Self {
                network_type: NetworkType::P2P,
                default_consensus_threshold: 2.0 / 3.0,
                default_timeout: 60,
                default_liveness_criteria_yes: true,
                max_rounds_override: None,
            },
        }
    }
}

impl From<ScopeConfig> for ConsensusConfig {
    fn from(config: ScopeConfig) -> Self {
        let (max_rounds, use_gossipsub_rounds) = match config.network_type {
            NetworkType::Gossipsub => (config.max_rounds_override.unwrap_or(2), true),
            // 0 triggers dynamic calculation for P2P networks
            NetworkType::P2P => (config.max_rounds_override.unwrap_or(0), false),
        };

        ConsensusConfig::new(
            config.default_consensus_threshold,
            config.default_timeout,
            max_rounds,
            use_gossipsub_rounds,
            config.default_liveness_criteria_yes,
        )
    }
}

pub struct ScopeConfigBuilder {
    config: ScopeConfig,
}

impl ScopeConfigBuilder {
    pub(crate) fn new() -> Self {
        Self {
            config: ScopeConfig::default(),
        }
    }

    /// Set network type (P2P or Gossipsub)
    pub fn with_network_type(mut self, network_type: NetworkType) -> Self {
        self.config.network_type = network_type;
        self
    }

    /// Set consensus threshold (0.0 to 1.0)
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.config.default_consensus_threshold = threshold;
        self
    }

    /// Set default timeout for proposals (in seconds)
    pub fn with_timeout(mut self, timeout: u64) -> Self {
        self.config.default_timeout = timeout;
        self
    }

    /// Set liveness criteria (how silent peers are counted)
    pub fn with_liveness_criteria(mut self, liveness_criteria_yes: bool) -> Self {
        self.config.default_liveness_criteria_yes = liveness_criteria_yes;
        self
    }

    /// Override max rounds (if None, uses network_type defaults)
    pub fn with_max_rounds(mut self, max_rounds: Option<u32>) -> Self {
        self.config.max_rounds_override = max_rounds;
        self
    }

    /// Set all configuration at once from a ScopeConfig
    pub fn with_config(mut self, config: ScopeConfig) -> Self {
        self.config = config;
        self
    }

    /// Start builder from an existing ScopeConfig (useful for partial updates)
    pub fn from_existing(config: ScopeConfig) -> Self {
        Self { config }
    }

    /// Use P2P preset with common defaults
    pub fn p2p_preset(mut self) -> Self {
        self.config.network_type = NetworkType::P2P;
        self.config.default_consensus_threshold = 2.0 / 3.0;
        self.config.default_timeout = 60;
        self.config.default_liveness_criteria_yes = true;
        self.config.max_rounds_override = None;
        self
    }

    /// Use Gossipsub preset with common defaults
    pub fn gossipsub_preset(mut self) -> Self {
        self.config.network_type = NetworkType::Gossipsub;
        self.config.default_consensus_threshold = 2.0 / 3.0;
        self.config.default_timeout = 60;
        self.config.default_liveness_criteria_yes = true;
        self.config.max_rounds_override = None;
        self
    }

    /// Use strict consensus (higher threshold = 0.9)
    pub fn strict_consensus(mut self) -> Self {
        self.config.default_consensus_threshold = 0.9;
        self
    }

    /// Use fast consensus (lower threshold = 0.6, shorter timeout = 30s)
    pub fn fast_consensus(mut self) -> Self {
        self.config.default_consensus_threshold = 0.6;
        self.config.default_timeout = 30;
        self
    }

    /// Start with network-specific defaults
    pub fn with_network_defaults(mut self, network_type: NetworkType) -> Self {
        match network_type {
            NetworkType::P2P => {
                self.config.network_type = NetworkType::P2P;
                self.config.default_consensus_threshold = 2.0 / 3.0;
                self.config.default_timeout = 60;
            }
            NetworkType::Gossipsub => {
                self.config.network_type = NetworkType::Gossipsub;
                self.config.default_consensus_threshold = 2.0 / 3.0;
                self.config.default_timeout = 60;
            }
        }
        self
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConsensusError> {
        self.config.validate()
    }

    /// Build the final ScopeConfig
    pub fn build(self) -> Result<ScopeConfig, ConsensusError> {
        self.validate()?;
        Ok(self.config)
    }

    /// Get the current configuration
    pub fn get_config(&self) -> ScopeConfig {
        self.config.clone()
    }
}

impl Default for ScopeConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}
