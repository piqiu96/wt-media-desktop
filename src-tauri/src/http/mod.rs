//! HTTP clients for the two surfaces Desktop talks to.
//!
//! They are separate types because they are trusted differently. The Local Agent
//! client targets a loopback port on this machine and is the one that will carry
//! the per-launch runtime token; the Cloud client is reached at a base URL the
//! caller supplies per request and authenticates with the node credential the
//! bind flow stored. One shared type for both meant a command could reach either
//! surface without saying which — and made "does this call carry the local
//! token?" unanswerable by looking at the type.

mod cloud;
mod local_agent;

pub use cloud::CloudClient;
pub use local_agent::LocalAgentClient;
