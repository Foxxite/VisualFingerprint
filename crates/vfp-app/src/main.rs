//! Desktop start-up. All code is in `lib.rs`, so the Android build can share it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    vfp_app_lib::run()
}
