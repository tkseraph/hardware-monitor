//! Platform selection happens here, never through runtime OS string checks.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub use macos::sampler::Sampler;
#[cfg(target_os = "windows")]
pub use windows::sampler::Sampler;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("Hardware Monitor currently targets macOS and Windows only");
