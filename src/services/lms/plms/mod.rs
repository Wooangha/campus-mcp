//! The PLMS backend: SSO login, then Moodle queries as a signed-in user.
//!
//! [`PlmsClient::connect`] signs in from a [`PlmsConfig`]; from there the client
//! is an [`Lms`](crate::services::lms::Lms).
//!
//! The submodules stack in the order a request travels through them:
//! `http` carries it, `navigate` decides what a browser would do with the
//! page that comes back, `page` scrapes that page, and `sso` drives the
//! login flow built on top of all three. `moodle` then queries PLMS.

mod config;
mod error;
mod http;
mod moodle;
mod navigate;
mod page;
mod sso;

pub use config::PlmsConfig;
pub use error::PlmsError;
pub use sso::PlmsClient;
