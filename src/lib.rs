//! Run an [Autumn](https://autumn-web.app) application on AWS Lambda.
//!
//! Add the plugin. Do not change the call to `run()`:
//!
//! ```rust,no_run
//! use autumn_plugin_aws_lambda::AwsLambdaPlugin;
//! use autumn_web::prelude::*;
//!
//! #[get("/")]
//! async fn index() -> &'static str {
//!     "hello"
//! }
//!
//! #[autumn_web::main]
//! async fn main() {
//!     autumn_web::app()
//!         .plugin(AwsLambdaPlugin::new())
//!         .routes(routes![index])
//!         .run()
//!         .await;
//! }
//! ```
//!
//! On Lambda, the plugin receives each event. It sends the event to the
//! Autumn server as an HTTP request on loopback. With the default
//! [`Activation::Auto`], the plugin stays idle outside Lambda. See the
//! [design record](https://github.com/autumn-foundation/autumn-plugin-aws-lambda/blob/main/docs/adr/0001-loopback-proxy.md).

mod activation;
mod error;
mod headers;
mod plugin;
mod proxy;
mod runtime;
mod timing;
mod upstream;

pub use activation::Activation;
pub use error::PluginError;
pub use plugin::AwsLambdaPlugin;
pub use runtime::ResponseMode;

/// Internal items for the tests of this crate.
///
/// This module is not part of the public API. It can change in any release.
#[doc(hidden)]
pub mod __private {
    pub use crate::activation::{RUNTIME_API_ENV, lambda_concurrency, missing_lambda_env};
    pub use crate::headers::{
        LAMBDA_REQUEST_ID_HEADER, fold_repeated_headers, forwarded_client_ip,
        prepare_request_headers, set_forwarded_for, strip_hop_by_hop, text_body_is_lossless,
        upstream_path_and_query,
    };
    pub use crate::plugin::slow_shutdown_settings;
    pub use crate::proxy::{LambdaProxy, ProxyBody, ProxyFailure};
    pub use crate::runtime::wait_until;
    pub use crate::timing::{invoke_budget_ms, next_delay_ms};
    pub use crate::upstream::Upstream;
}
