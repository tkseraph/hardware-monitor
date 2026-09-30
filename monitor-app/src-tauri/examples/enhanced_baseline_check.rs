//! Explicit ordinary-user enumeration through the pipeline; no tracing or elevation.
#[cfg(target_os = "windows")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use app_lib::enhanced::{
        clock_bridge::HostClock, disk_io::Limits, enumeration, pipeline::Pipeline,
    };
    let mut clock = HostClock::new()?;
    let mut pipeline = Pipeline::new(Limits::default(), 8192);
    let epoch = pipeline
        .begin_baseline(clock.now_ns()?)
        .map_err(|_| "baseline start failed")?;
    let candidate = enumeration::capture(&mut clock, Limits::default())?;
    pipeline
        .install_candidate(epoch, &candidate)
        .map_err(|_| "baseline install failed")?;
    pipeline.stop();
    println!(
        "{}",
        serde_json::json!({"coverage":candidate.coverage,"capture_ms":(candidate.end_ns-candidate.start_ns)/1_000_000,"verified_scope_installed":true,"pipeline_stopped":true,"systemwide_stream_ready":false})
    );
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn main() {
    println!("Windows only");
}
