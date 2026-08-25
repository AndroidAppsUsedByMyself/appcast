//! Core layer: transporter abstraction, registry, shared config and errors.
//!
//! The core never depends on any UI library — frontends convert user input
//! into a [`ResolvedConfig`](transporter::ResolvedConfig) and drive a
//! [`Transporter`](transporter::Transporter) through the registry.

pub mod error;
pub mod plugins;
pub mod registry;
pub mod transporter;
pub mod transporters;
