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
//!
//! The pieces, in the order a record meets them: [`targets`] decides whether
//! the record is Desktop's at all, [`backend`] renders it as one line and
//! hands it to a sink, [`redact`] masks what may not be printed on the way, and
//! [`rolling`] bounds what the files may occupy. [`paths`] says where they are,
//! and [`setup`] is the one function that puts it all together for the process.

pub mod backend;
pub mod paths;
pub mod redact;
pub mod rolling;
pub mod setup;
pub mod targets;
#[cfg(test)]
pub(crate) mod test_support;
