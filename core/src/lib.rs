//! Core library for Spite: Microsoft Graph sync, the local mail store,
//! secret handling, and offline logic all live here. The Tauri shell
//! (`src-tauri`) is a thin IPC layer over this crate.

pub mod auth;
pub mod config;
pub mod graph;
pub mod store;
pub mod sync;

pub const APP_NAME: &str = "Spite";

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_spite() {
        assert_eq!(super::APP_NAME, "Spite");
    }
}
