//! unread: code coverage for human eyes.
//!
//! Every line of a repository gets one of three labels, and every label cites its evidence:
//!
//! * `human`  a person wrote it (no evidence that an agent did)
//! * `seen`   an agent wrote it and there is evidence a person looked at it
//! * `unread` an agent wrote it and nothing shows that a person ever looked
//!
//! The pipeline is [`sources`] (what agents wrote, from their own session logs) plus [`git`]
//! (who committed what) plus [`store`] (what the team recorded in the repository), joined in
//! [`analyze`].

pub mod analyze;
pub mod attribution;
pub mod git;
pub mod github;
pub mod ignore;
pub mod lines;
pub mod model;
pub mod patch;
pub mod render;
pub mod review;
pub mod risk;
pub mod sources;
pub mod store;
pub mod treemap;
pub mod util;
