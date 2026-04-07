//! ONDE Core — Réseau de Résilience Citoyen
//!
//! Core network engine for the mesh resilience network.
//! Provides DTN routing, PoW antispam, Nostr protocol,
//! cryptography, AI inference, and offline storage.

pub mod network;
pub mod protocol;
pub mod crypto;
pub mod storage;
pub mod ai;
pub mod node;

// Re-exports for convenience
pub use dtn_router as dtn;
pub use llm_inference as ai_engine;
pub use node::{Node, NodeConfig, NodeType};