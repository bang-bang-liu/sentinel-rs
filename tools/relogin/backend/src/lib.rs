pub mod accounts;
pub mod api;
pub mod config;
pub mod login;
pub mod mail;
pub mod tasks;

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

pub fn now() -> i64 {
    sentinel_rs::flow::store::now_ts()
}
