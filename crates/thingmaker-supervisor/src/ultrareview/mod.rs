//! Ultra-Review: several models from different providers review one change
//! on their own, then cross-examine each other's findings, with evidence,
//! until only what holds up is left; a Chair writes the verdict.
//!
//! - [`model`]: the review, its panel, findings, threads and verdict;
//! - [`debate`]: the rules, as plain functions;
//! - [`prompt`] and [`parse`]: what reviewers are asked, and reading their answers;
//! - [`checkout`]: the change, checked out for the reviewers, and `gh`;
//! - [`engine`]: the service that runs a review through delegation;
//! - [`markdown`]: the verdict, written up for the pull request.

pub mod checkout;
pub mod debate;
pub mod engine;
pub mod markdown;
pub mod model;
pub mod parse;
pub mod prompt;

pub use engine::{DelegationRunner, Runner, StartRequest, UltraReviewService};
pub use model::Review;
