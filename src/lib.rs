//! Run an [Autumn](https://autumn-web.app) application on AWS Lambda.
//!
//! Add the plugin. Keep `run()`:
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
//! On Lambda, the plugin receives each event and sends it to the local
//! Autumn server as an HTTP request. Outside Lambda, the plugin does
//! nothing. See `docs/adr/0001-loopback-proxy.md`.

mod activation;
mod error;
mod headers;
mod plugin;
mod proxy;
mod runtime;
mod timing;
mod upstream;

pub use activation::{Activation, RUNTIME_API_ENV};
pub use error::PluginError;
pub use headers::{strip_hop_by_hop, prepare_request_headers, REQUEST_ID_HEADER};
pub use plugin::AwsLambdaPlugin;
pub use proxy::{LambdaProxy, ProxyBody, ProxyFailure};
pub use runtime::{ResponseMode, wait_until};
pub use timing::{invoke_budget_ms, next_delay_ms};
pub use upstream::Upstream;
