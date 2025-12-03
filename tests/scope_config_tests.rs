use hashgraph_like_consensus::{
    error::ConsensusError, scope::ScopeID, scope_config::NetworkType,
    service::DefaultConsensusService,
};

const SCOPE_NAME: &str = "test_scope";

#[tokio::test]
async fn test_scope_config_creation() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from(SCOPE_NAME);

    // Initialize scope with P2P network and custom threshold
    service
        .scope(&scope)
        .await
        .unwrap()
        .with_network_type(NetworkType::P2P)
        .with_threshold(0.75)
        .with_timeout(120)
        .with_liveness_criteria(true)
        .initialize()
        .await
        .unwrap();

    // Verify the configuration was saved
    let config = service.scope(&scope).await.unwrap().get_config();

    assert_eq!(config.network_type, NetworkType::P2P);
    assert_eq!(config.default_consensus_threshold, 0.75);
    assert_eq!(config.default_timeout, 120);
    assert!(config.default_liveness_criteria_yes);
}

#[tokio::test]
async fn test_scope_config_update() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("update_test_scope");

    // Initialize with default config
    service
        .scope(&scope)
        .await
        .unwrap()
        .with_network_type(NetworkType::Gossipsub)
        .with_threshold(2.0 / 3.0)
        .with_timeout(60)
        .initialize()
        .await
        .unwrap();

    // Update only threshold
    service
        .scope(&scope)
        .await
        .unwrap()
        .with_threshold(0.8)
        .update()
        .await
        .unwrap();

    // Verify threshold was updated, but other fields remain unchanged
    let config = service.scope(&scope).await.unwrap().get_config();

    assert_eq!(config.default_consensus_threshold, 0.8);
    assert_eq!(config.network_type, NetworkType::Gossipsub); // Should remain unchanged
    assert_eq!(config.default_timeout, 60); // Should remain unchanged
}

#[tokio::test]
async fn test_scope_config_update_multiple_fields() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("multi_update_scope");

    // Initialize with initial config
    service
        .scope(&scope)
        .await
        .unwrap()
        .with_network_type(NetworkType::P2P)
        .with_threshold(0.6)
        .with_timeout(30)
        .initialize()
        .await
        .unwrap();

    // Update multiple fields at once
    service
        .scope(&scope)
        .await
        .unwrap()
        .with_threshold(0.9)
        .with_timeout(180)
        .with_liveness_criteria(false)
        .update()
        .await
        .unwrap();

    // Verify all fields were updated
    let config = service.scope(&scope).await.unwrap().get_config();

    assert_eq!(config.default_consensus_threshold, 0.9);
    assert_eq!(config.default_timeout, 180);
    assert!(!config.default_liveness_criteria_yes);
    assert_eq!(config.network_type, NetworkType::P2P); // Should remain unchanged
}

#[tokio::test]
async fn test_scope_config_presets() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("preset_test_scope");

    // Test P2P preset
    service
        .scope(&scope)
        .await
        .unwrap()
        .p2p_preset()
        .initialize()
        .await
        .unwrap();

    let config = service.scope(&scope).await.unwrap().get_config();

    assert_eq!(config.network_type, NetworkType::P2P);
    assert_eq!(config.default_consensus_threshold, 2.0 / 3.0);
    assert_eq!(config.default_timeout, 60);

    // Test Gossipsub preset
    service
        .scope(&scope)
        .await
        .unwrap()
        .gossipsub_preset()
        .update()
        .await
        .unwrap();

    let config = service.scope(&scope).await.unwrap().get_config();

    assert_eq!(config.network_type, NetworkType::Gossipsub);
}

#[tokio::test]
async fn test_scope_config_validation() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("validation_test_scope");

    // Test invalid threshold (too high)
    let result = service
        .scope(&scope)
        .await
        .unwrap()
        .with_threshold(1.5) // Invalid: > 1.0
        .initialize()
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ConsensusError::InvalidConsensusThreshold(_)
    ));

    // Test invalid threshold (negative)
    let result = service
        .scope(&scope)
        .await
        .unwrap()
        .with_threshold(-0.1) // Invalid: < 0.0
        .initialize()
        .await;

    assert!(result.is_err());

    // Test invalid timeout (zero)
    let result = service
        .scope(&scope)
        .await
        .unwrap()
        .with_timeout(0) // Invalid: must be > 0
        .initialize()
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ConsensusError::InvalidProposalConfiguration(_)
    ));
}

#[tokio::test]
async fn test_scope_config_new_scope_uses_defaults() {
    let service = DefaultConsensusService::default();
    let scope = ScopeID::from("new_scope_defaults");

    // Get config for non-existent scope - should return defaults
    let config = service.scope(&scope).await.unwrap().get_config();

    // Should have default values
    assert_eq!(config.network_type, NetworkType::Gossipsub);
    assert_eq!(config.default_consensus_threshold, 2.0 / 3.0);
    assert_eq!(config.default_timeout, 60);
    assert!(config.default_liveness_criteria_yes);
}

#[tokio::test]
async fn test_max_rounds_override_zero_validation() {
    let service = DefaultConsensusService::default();
    let scope_p2p = ScopeID::from("p2p_zero_rounds");
    let scope_gossipsub = ScopeID::from("gossipsub_zero_rounds");

    // Test that max_rounds_override = Some(0) is allowed for P2P networks
    // (0 triggers dynamic calculation based on consensus threshold)
    let result = service
        .scope(&scope_p2p)
        .await
        .unwrap()
        .with_network_type(NetworkType::P2P)
        .with_max_rounds(Some(0))
        .initialize()
        .await;

    assert!(
        result.is_ok(),
        "max_rounds_override = Some(0) should be allowed for P2P networks"
    );

    let config = service.scope(&scope_p2p).await.unwrap().get_config();
    assert_eq!(config.max_rounds_override, Some(0));
    assert_eq!(config.network_type, NetworkType::P2P);

    // Test that max_rounds_override = Some(0) is rejected for Gossipsub networks
    let result = service
        .scope(&scope_gossipsub)
        .await
        .unwrap()
        .with_network_type(NetworkType::Gossipsub)
        .with_max_rounds(Some(0))
        .initialize()
        .await;

    assert!(
        result.is_err(),
        "max_rounds_override = Some(0) should be rejected for Gossipsub networks"
    );
    assert!(matches!(
        result.unwrap_err(),
        ConsensusError::InvalidProposalConfiguration(_)
    ));
}
