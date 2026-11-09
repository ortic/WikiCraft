//! WikiCraft server: HTTP API, real-time collaboration, search and importers.

pub mod app;
pub mod audit;
pub mod auth;
pub mod authz;
pub mod blobs;
pub mod cluster;
pub mod collab;
pub mod config;
pub mod error;
pub mod export;
pub mod extract;
pub mod importer;
pub mod jobs;
pub mod mail;
pub mod mcp;
pub mod metrics;
pub mod routes;
pub mod search;
pub mod sso;
