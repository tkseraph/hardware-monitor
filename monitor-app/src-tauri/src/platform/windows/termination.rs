//! Windows has no general SIGTERM equivalent. This endpoint never kills.
pub fn request(_pid: u32, _start_marker: u64) -> Result<(), String> {
    Err("not_implemented".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_cannot_terminate_any_process() {
        for pid in [0, 1, std::process::id(), u32::MAX] {
            assert_eq!(super::request(pid, 123).unwrap_err(), "not_implemented");
        }
    }
}
