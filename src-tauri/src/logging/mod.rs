//! Desktop's own logging: where it goes, how it rolls, and what it keeps.
//!
//! Desktop logs what **Desktop** does — startup and exit, the config it loaded,
//! starting and stopping the Agent, Agent health checks, the sidecar exiting
//! unexpectedly. It does not log the Agent's business: that is the Agent's own
//! three files in the Agent's own directory, and copying them here would pay
//! twice for the same record and give the reader two places to look (the user's
//! ruling 七, which is checked in `targets`).
//!
//! The pieces are separated the same way `paths` and `config` are: the rules are
//! pure functions of injected inputs — an environment, a home directory, a
//! manifest directory, a clock — and the code that touches the filesystem is
//! thin enough to read in one sitting. A logging system that cannot be tested
//! without writing files ends up tested only by the launch that needed it.

pub mod paths;
pub mod rolling;
